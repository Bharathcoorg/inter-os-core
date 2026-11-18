# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Design-invariant contract for the serial trust / panic-mutation boundary.

The companion suite `test_serial_debug_production_exclusion.py` proves the gated
verbs are compiled OUT of a production binary (source gates + byte scan). This
suite pins the *design decisions* that make that exclusion correct rather than
merely absent, so a future edit cannot quietly undo them:

  1. The gate is a production EXCLUSION, not a deletion of legitimate
     provisioning. RAPP_TRUST still journals a publisher root (trust_key with the
     in-memory grant fallback) and still refuses a system/platform identity, and
     the boot path still restores provisioned roots (REVOCATION_REGISTRY.seed).
     Provision once on a debug-control-plane build; a production image keeps the
     roots it was provisioned with but cannot mint new ones over COM1.

  2. Gating did not WEAKEN any existing safety check inside the TLS mutation
     handlers: the anchor DER size bound, the chunk-bounds check, the
     incomplete-anchor rejection at commit, and the install/revoke calls all
     survive verbatim.

  3. The boundary is COMPILE-TIME only. There is no runtime "trusted caller",
     "factory window" or "provision unlocked" flag that any serial line could set
     to re-enable trust mutation -- that would not be a boundary.

  4. The read-only observers stay ungated: TLS_TRUST_LIST observes the store and
     cannot change it, so it remains available in every build.

These are source-text pins. The runtime state-unchanged proofs live in the Rust
unit tests `production_build_refuses_trust_and_panic_mutations` (default build)
and `debug_control_plane_trust_and_panic_mutations_work` (feature build).
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERIAL_RS = ROOT / "kernel" / "src" / "serial.rs"
MAIN_RS = ROOT / "kernel" / "src" / "main.rs"

CFG_GATE = '#[cfg(feature = "debug-control-plane")]'
CFG_NOT_GATE = '#[cfg(not(feature = "debug-control-plane"))]'


def _serial_text() -> str:
    return SERIAL_RS.read_text(encoding="utf-8")


def _prod_region(text: str) -> str:
    """Production code only: the #[cfg(test)] module legitimately names gated
    verbs and bypass-shaped identifiers in order to assert their behaviour."""
    return text.split("mod tests {", 1)[0]


def _nearest_code_line_above(lines, idx: int) -> str:
    j = idx - 1
    while j >= 0:
        s = lines[j].strip()
        if s == "" or s.startswith("//"):
            j -= 1
            continue
        break
    return lines[j].strip() if j >= 0 else ""


def test_rapp_trust_provisioning_path_is_preserved_not_deleted():
    text = _serial_text()
    # The gated handler still journals a root and keeps the in-memory fallback that
    # lets the trust decision be observed on a host with no block device bound.
    assert "reg.trust_key(key, publisher)" in text, (
        "RAPP_TRUST no longer journals the publisher root via trust_key; the gate "
        "deleted legitimate provisioning instead of excluding it from production"
    )
    assert "reg.grant_trust_root(key, publisher)" in text, (
        "RAPP_TRUST lost its in-memory grant fallback"
    )
    # The existing least-privilege check must survive: a platform identity cannot be
    # minted from the unauthenticated console even in a debug build.
    assert "is_system_publisher_identity(publisher)" in text, (
        "RAPP_TRUST no longer refuses a system/platform publisher identity"
    )
    assert "system publisher cannot be provisioned via serial" in text, (
        "RAPP_TRUST system-publisher refusal wording was removed"
    )
    # Boot restores the provisioned roots, which is what makes once-only provisioning
    # sound: nothing on the boot path mints a new root.
    main = MAIN_RS.read_text(encoding="utf-8")
    assert "REVOCATION_REGISTRY.lock().seed()" in main, (
        "boot no longer restores provisioned publisher trust roots (seed); the "
        "once-only provisioning model that justifies gating RAPP_TRUST is broken"
    )


def test_tls_mutation_handlers_keep_their_existing_safety_checks():
    text = _serial_text()
    required = (
        "len > crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES",
        "offset.checked_add(chunk_len)",
        "state.received[..state.expected_len].iter().any(|v| !*v)",
        "transfer.take()",
        "store.install(der, label)",
        "store.revoke(position)",
    )
    missing = [s for s in required if s not in text]
    assert not missing, (
        f"gating weakened or removed existing TLS trust safety checks: {missing}. "
        "The anchor size bound, chunk-bounds check, incomplete-anchor rejection and "
        "install/revoke calls must survive verbatim inside the gated handlers."
    )


def test_trust_boundary_is_compile_time_only_no_runtime_enable_flag():
    prod = _prod_region(_serial_text())
    # A runtime flag any serial line could set is NOT a boundary. Pin the absence of
    # the plausible bypass identifiers so a future edit cannot reintroduce one.
    forbidden = (
        "trusted_caller", "TRUSTED_CALLER",
        "factory_window", "FACTORY_WINDOW",
        "provision_unlocked", "PROVISION_UNLOCKED",
        "provision_mode", "PROVISION_MODE",
        "trust_enabled", "TRUST_ENABLED",
        "debug_unlocked", "DEBUG_UNLOCKED",
        "serial_authenticated", "SERIAL_AUTHENTICATED",
    )
    found = [tok for tok in forbidden if tok in prod]
    assert not found, (
        f"serial.rs production code introduces a runtime trust-enable bypass {found}; "
        "the trust mutation boundary must be the compile-time debug-control-plane "
        "feature only, never a flag a console line can set"
    )
    # The mutating handlers are guarded by the cfg feature, not a runtime `if`.
    lines = prod.splitlines()
    for verb in ("RAPP_TRUST", "TLS_TRUST_BEGIN", "TLS_TRUST_COMMIT", "TLS_TRUST_REVOKE"):
        needle = f'if verb.eq_ignore_ascii_case("{verb}")'
        idx = next((i for i, l in enumerate(lines) if needle in l), None)
        assert idx is not None, f"{verb} handler not found"
        assert _nearest_code_line_above(lines, idx) == CFG_GATE, (
            f"{verb} is not guarded by the compile-time feature gate"
        )


def test_tls_trust_list_stays_ungated_readonly():
    lines = _serial_text().splitlines()
    needle = 'if verb.eq_ignore_ascii_case("TLS_TRUST_LIST")'
    idx = next((i for i, l in enumerate(lines) if needle in l), None)
    assert idx is not None, "TLS_TRUST_LIST handler not found in serial.rs"
    above = _nearest_code_line_above(lines, idx)
    assert above not in (CFG_GATE, CFG_NOT_GATE), (
        "TLS_TRUST_LIST (read-only store observer) was feature-gated; it must stay "
        "available in production because it cannot mutate the store"
    )
    # It only reads: the listing handler must not call install/revoke.
    block = "\n".join(lines[idx:idx + 12])
    assert "store.install(" not in block and "store.revoke(" not in block, (
        "TLS_TRUST_LIST handler mutates the store; it must remain read-only"
    )