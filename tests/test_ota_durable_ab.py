# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Adversarial contract for the durable A/B update subsystem.

The A/B store is the one subsystem whose failure mode is *silence*. A torn
metadata write, a stale copy selection or an image that was published before
its payload all end the same way: the device boots something nobody can
prove, and nothing in the logs says so. The Rust suite in
`crates/interos-ota/src/tests.rs` proves the behaviour of the state machine
on a host; the rules below are the ones that a *reader of the source* has to
be able to check, because they are about where one statement sits relative to
another and about what the crate is allowed to depend on.

Each case below reads the crate's own source and names one rule; each "can
fail" case puts that rule back the way it was and proves the reader sees the
fault. The rules are:

  * the crate is `no_std` and depends on nothing outside itself, so it can be
    linked into a microkernel that has no standard library;
  * the engine adopts a state only after the device confirmed it, so a failed
    commit cannot leave memory claiming what the disk does not hold;
  * the store reads both metadata copies before it chooses which one to
    overwrite, so a commit never writes generation one over the record the
    reader would select;
  * the slot header is written after the payload and carries the payload's
    digest, so an image that was published before its bytes landed is not
    believed;
  * anti-rollback compares against the newest version the device has ever
    accepted, not against the version that happens to be active;
  * the boot policy never selects a slot that is unverified or corrupted.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CRATE = ROOT / "crates" / "interos-ota"
SRC = CRATE / "src"


def source(name: str) -> str:
    return (SRC / name).read_text(encoding="utf-8")


def manifest() -> str:
    return (CRATE / "Cargo.toml").read_text(encoding="utf-8")


def function_body(text: str, signature: str) -> str:
    """The body of the function whose line starts with `signature`.

    A trait declares the same signature as the implementation that follows
    it, and the declaration has no body, so the occurrence that is followed by
    a brace is the one that is read.
    """
    start = -1
    while True:
        found = text.find(signature, start + 1)
        if found < 0:
            raise AssertionError(f"no function starting with {signature!r}")
        start = found
        if text[found + len(signature):].lstrip().startswith("{"):
            break
    brace = text.index("{", start)
    depth = 0
    for index in range(brace, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return text[brace:index]
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


def _dependency_faults(crate_manifest: str, lib: str) -> list:
    """Every way the crate could stop being linkable into a microkernel."""
    faults = []

    if "#![no_std]" not in lib:
        faults.append("the crate no longer declares itself no_std")

    # The dependency table must be empty or hold only what the crate brings
    # with it. A third-party crate in the constrained core is forbidden.
    table = re.search(r"\[dependencies\](.*?)(\n\[|\Z)", crate_manifest, re.S)
    if table is None:
        return faults
    entries = [
        line.strip()
        for line in table.group(1).splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]
    for entry in entries:
        faults.append(f"the crate depends on {entry!r}: the constrained core takes no third-party crates")

    return faults


def _adoption_faults(engine: str) -> list:
    """Every way the engine could claim a state the device does not hold."""
    faults = []
    body = without_comments(
        function_body(engine, "fn commit(&mut self, candidate: OtaState) -> Result<(), OtaError>")
    )

    save_at = body.find("self.store.save(&candidate)")
    adopt_at = body.find("self.state = candidate")
    if save_at < 0:
        faults.append("the engine no longer writes the candidate to the device")
        return faults
    if adopt_at < 0:
        faults.append("the engine never adopts the candidate state")
        return faults
    if adopt_at < save_at:
        faults.append(
            "the engine adopts the candidate state before the device confirmed it"
        )
    return faults


def _target_copy_faults(store: str) -> list:
    """Every way a commit could overwrite the record the reader would select."""
    faults = []
    body = without_comments(
        function_body(store, "fn save(&mut self, state: &OtaState) -> Result<(), OtaError>")
    )

    read_at = body.find("self.read_copy(0)?")
    second_read_at = body.find("self.read_copy(1)?")
    target_at = body.find("self.target_copy()")

    if read_at < 0 or second_read_at < 0:
        faults.append(
            "the store no longer reads both metadata copies before it chooses a target"
        )
        return faults
    if target_at < 0:
        faults.append("the store no longer chooses a target copy")
        return faults
    if target_at < max(read_at, second_read_at):
        faults.append(
            "the store chooses which copy to overwrite from a view it has not read back"
        )
    return faults


def _slot_publish_faults(store: str) -> list:
    """Every way an image could be published before its bytes landed."""
    faults = []
    body = without_comments(
        function_body(
            store, "fn write_slot(&mut self, slot: Slot, image: &[u8]) -> Result<(), OtaError>"
        )
    )

    payload_at = body.find("self.write_sector(payload_base + index as u32, &sector)?")
    header_at = body.find("self.write_sector(self.layout.slot_base(slot), &header)?")
    digest_at = body.find("sha256(image)")

    if payload_at < 0:
        faults.append("the store no longer writes the image's bytes")
    if header_at < 0:
        faults.append("the store no longer writes the slot header")
    if digest_at < 0:
        faults.append("the slot header no longer carries the image's digest")
    if payload_at >= 0 and header_at >= 0 and header_at < payload_at:
        faults.append("the slot header is published before the image's bytes are written")

    # The reader has to re-hash the bytes that come back, or a torn image is
    # indistinguishable from a whole one.
    read_body = without_comments(
        function_body(
            store, "fn read_slot(&mut self, slot: Slot) -> Result<Option<Vec<u8>>, OtaError>"
        )
    )
    if "sha256(&out)" not in read_body:
        faults.append("the slot reader no longer hashes the bytes it read back")
    if "SlotDigestMismatch" not in read_body:
        faults.append("a slot whose bytes do not match its header is not refused")

    return faults


def _rollback_faults(policy: str, state: str) -> list:
    """Every way an older version could be installed again."""
    faults = []
    body = without_comments(
        function_body(policy, "pub fn accepts_version(state: &OtaState, version: u32) -> bool")
    )

    if "state.highest_version()" not in body:
        faults.append("anti-rollback no longer compares against the newest version the device accepted")
        return faults
    if "active_slot" in body:
        faults.append("anti-rollback compares against the active slot instead of the newest version")

    # The floor has to be the newest of both slots, or a rollback re-opens the
    # downgrade the rollback was performed to stop.
    highest = without_comments(function_body(state, "pub fn highest_version(&self) -> u32"))
    if "max" not in highest:
        faults.append("the anti-rollback floor is not the maximum over both slots")
    return faults


def _boot_policy_faults(policy: str) -> list:
    """Every way an unproven slot could be booted."""
    faults = []
    body = without_comments(
        function_body(policy, "pub fn select_boot_slot(state: &OtaState) -> Option<Slot>")
    )

    if "state.slot(active).status.is_bootable()" not in body:
        faults.append("the boot policy no longer asks whether the active slot may be booted")
        return faults
    if "state.slot(alternate).status.is_bootable()" not in body:
        faults.append("the boot policy no longer asks whether the other slot may be booted")
        return faults
    if "SlotStatus::Unverified" in body or "SlotStatus::Corrupted" in body:
        faults.append("the boot policy names an unverified or corrupted slot directly")
    return faults


def test_the_ota_crate_is_no_std_and_depends_on_nothing():
    """The crate can be linked into a microkernel that has no std."""
    faults = _dependency_faults(manifest(), source("lib.rs"))
    assert not faults, faults


def test_the_engine_adopts_a_state_only_after_the_device_confirmed_it():
    """A failed commit cannot leave memory claiming what the disk does not hold."""
    faults = _adoption_faults(source("engine.rs"))
    assert not faults, faults


def test_the_store_reads_the_device_before_it_overwrites_a_metadata_copy():
    """A commit never writes generation one over the record the reader selects."""
    faults = _target_copy_faults(source("store.rs"))
    assert not faults, faults


def test_the_slot_header_is_published_after_the_payload_and_names_its_digest():
    """An image that was published before its bytes landed is not believed."""
    faults = _slot_publish_faults(source("store.rs"))
    assert not faults, faults


def test_anti_rollback_compares_against_the_newest_version_the_device_accepted():
    """A rollback does not re-open the downgrade it was performed to stop."""
    faults = _rollback_faults(source("policy.rs"), source("state.rs"))
    assert not faults, faults


def test_the_boot_policy_never_selects_an_unproven_slot():
    """An interrupted update leaves a candidate that must not be booted."""
    faults = _boot_policy_faults(source("policy.rs"))
    assert not faults, faults


def test_the_durable_ab_contract_can_fail():
    """Each rule is put back the way it was, and the reader has to see it.

    A detector that cannot be tripped is a comment, so every rule above is
    checked against a copy of the crate with that rule reversed.
    """
    engine = source("engine.rs")
    store = source("store.rs")
    policy = source("policy.rs")
    state = source("state.rs")
    crate_manifest = manifest()
    lib = source("lib.rs")

    assert not _dependency_faults(crate_manifest, lib)
    assert not _adoption_faults(engine)
    assert not _target_copy_faults(store)
    assert not _slot_publish_faults(store)
    assert not _rollback_faults(policy, state)
    assert not _boot_policy_faults(policy)

    # 1. The crate grows a third-party dependency.
    dependent = crate_manifest.replace(
        "[dependencies]",
        "[dependencies]\nserde = \"1\"",
        1,
    )
    assert dependent != crate_manifest, "the dependency table the detector watches moved; this fixture is stale"
    assert any(
        "depends on" in fault for fault in _dependency_faults(dependent, lib)
    ), "a crate with a third-party dependency passed"

    # 2. The engine adopts the state before the device confirmed it.
    early = engine.replace(
        """        match self.store.save(&candidate) {
            Ok(()) => {
                self.state = candidate;""",
        """        self.state = candidate;
        match self.store.save(&candidate) {
            Ok(()) => {""",
        1,
    )
    assert early != engine, "the commit the detector watches moved; this fixture is stale"
    assert any(
        "adopts the candidate state before" in fault for fault in _adoption_faults(early)
    ), "an engine that adopts before the write passed"

    # 3. The store picks its target from a view it never read back.
    stale = store.replace(
        """        let copy_a = self.read_copy(0)?;
        let copy_b = self.read_copy(1)?;
        self.copies = [copy_a, copy_b];""",
        "",
        1,
    )
    assert stale != store, "the copy read the detector watches moved; this fixture is stale"
    assert any(
        "no longer reads both metadata copies" in fault for fault in _target_copy_faults(stale)
    ), "a store that never read the device passed"

    # 4. The slot header is published before the image's bytes are written.
    early_header = store.replace(
        """        let payload_base = self.layout.slot_payload_base(slot);
        let mut sector = [0u8; SECTOR_SIZE];
        let chunks: Vec<&[u8]> = image.chunks(SECTOR_SIZE).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            sector.fill(0);
            sector[..chunk.len()].copy_from_slice(chunk);
            self.write_sector(payload_base + index as u32, &sector)?;
        }
        self.flush()?;

        let header = encode_slot_header(image.len() as u32, sha256(image));
        self.write_sector(self.layout.slot_base(slot), &header)?;
        self.flush()?;
        Ok(())""",
        """        let header = encode_slot_header(image.len() as u32, sha256(image));
        self.write_sector(self.layout.slot_base(slot), &header)?;
        self.flush()?;

        let payload_base = self.layout.slot_payload_base(slot);
        let mut sector = [0u8; SECTOR_SIZE];
        let chunks: Vec<&[u8]> = image.chunks(SECTOR_SIZE).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            sector.fill(0);
            sector[..chunk.len()].copy_from_slice(chunk);
            self.write_sector(payload_base + index as u32, &sector)?;
        }
        self.flush()?;
        Ok(())""",
        1,
    )
    assert early_header != store, "the slot write the detector watches moved; this fixture is stale"
    assert any(
        "published before the image's bytes" in fault for fault in _slot_publish_faults(early_header)
    ), "a store that publishes the header first passed"

    # 5. Anti-rollback compares against the active slot only.
    active_only = policy.replace(
        "version > state.highest_version()",
        "version > state.slot(state.active_slot).version",
        1,
    )
    assert active_only != policy, "the rollback floor the detector watches moved; this fixture is stale"
    assert any(
        "no longer compares against the newest version" in fault
        for fault in _rollback_faults(active_only, state)
    ), "an anti-rollback floor of one slot passed"

    # 6. The boot policy boots whatever is active.
    boot_anything = policy.replace(
        """    if state.slot(active).status.is_bootable() {
        return Some(active);
    }""",
        """    return Some(active);""",
        1,
    )
    assert boot_anything != policy, "the boot policy the detector watches moved; this fixture is stale"
    assert any(
        "no longer asks whether the active slot may be booted" in fault
        for fault in _boot_policy_faults(boot_anything)
    ), "a boot policy that boots anything passed"
