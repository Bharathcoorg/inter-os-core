"""Truth contract for the InterOS vault application.

A vault is the one surface where a claim that the platform did not make
is most dangerous: a "keystore online" banner and a "Encrypted" badge
both tell the user their data is protected when the platform may have
refused every request. Every assertion below pins a specific defect
found by reading the app.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def vault_main() -> str:
    return read("apps/vault/src/main.rs")


def test_the_banner_does_not_announce_a_keystore_that_was_never_probed():
    """The startup banner said the keystore was online.

    The banner was written before the app had asked, and the kernel
    refuses to export raw key material to a Ring 3 app, so the claim was
    false on this platform. The banner now says the app is probing, and
    the result is reported after the probe.
    """
    source = vault_main()
    assert "Keystore online" not in source
    assert "probing the platform's storage and keystore" in source
    assert "if self.keys_derived() {" in source
    assert "no key is claimed" in source


def test_the_status_frame_is_not_called_a_backup():
    """The control was labelled "Export backup".

    What it writes is two counters and a checksum, in the clear, to the
    settings store. It is not a copy of the vault and it is not
    encrypted, so the label overstated what the user gets.
    """
    source = vault_main()
    assert "Export backup" not in source
    assert 'b"Write status frame"' in source
    assert 'b"STATUS FRAME & ENTROPY"' in source
    assert "counters and checksum, in the clear" in source


def test_the_filesystem_capability_is_resolved_by_its_type():
    """The identity read used the literal slot 3, which holds this app's
    write capability.

    The VFS verifies the capability's type at the slot it is given, so
    the read was refused before the file was opened and the identity read
    as unreadable on a device that has the file.
    """
    source = vault_main()
    assert "std::fs_open(3," not in source
    assert "std::fs_read(3," not in source
    assert "std::cap::fs_read_slot()" in source
    assert "std::cap::fs_write_slot()" in source


def test_the_display_capability_is_resolved_by_its_type():
    """The surface was created through the literal slot 6, which holds
    this app's IPC receive capability rather than its framebuffer."""
    source = vault_main()
    assert "surface_create(6," not in source
    assert "surface_flip(6)" not in source
    assert "surface_destroy(6," not in source
    assert "std::cap::framebuffer_slot()" in source


def test_the_keystore_slot_is_not_confused_with_a_capability_slot():
    """The keystore argument is a keystore slot, not a capability slot."""
    source = vault_main()
    assert "pub const ROOT_KEY_SLOT: u32 = 0;" in source
    assert "pub const STORAGE_KEY_SLOT: u32 = 1;" in source
    assert "std::keystore_derive(ROOT_KEY_SLOT," in source
    assert "std::keystore_derive(STORAGE_KEY_SLOT," in source


def test_the_home_gesture_only_leaves_the_app_from_the_home_strip():
    """A pan or a fling from anywhere returned to the launcher."""
    source = vault_main()
    assert "let is_home_gesture = input.y >= 1400 && input.x >= 240 && input.x <= 480;" in source
    assert "if is_home_gesture" in source


def test_no_lock_badge_appears_without_a_platform_confirmation():
    """A lock icon or an "Encrypted" badge must not be drawn unless the
    platform confirmed the item's state.

    The only mark the app draws is the round-trip verdict, and the code
    sets it only after an encrypt, a decrypt and a comparison all
    succeeded.
    """
    source = vault_main()
    for badge in ('b"Encrypted"', 'b"Locked"', 'b"Unlocked"', 'b"Sealed"'):
        assert badge not in source
    assert 'b"round-trip verified"' in source
