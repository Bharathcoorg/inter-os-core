"""Truth contract for the InterOS vault application.

A vault is the one surface where a claim that the platform did not make
is most dangerous: a "keystore online" banner and a "Encrypted" badge
both tell the user their data is protected when the platform may have
refused every request. Every assertion below pins a specific defect
found by reading the app.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The SDK functions that used to take a capability slot as their first
# argument. The kernel now authorises every one of them by the
# capability's *type*, so the app names the authority it wants and the
# kernel finds it.
SLOT_ARG_SDK_CALLS = (
    "app_storage_open",
    "display_set_mode",
    "display_set_orientation",
    "fs_list",
    "fs_open",
    "fs_read",
    "fs_stat",
    "fs_unlink",
    "fs_write",
    "get_ticks",
    "ipc_recv",
    "ipc_send",
    "log",
    "package_install",
    "package_list",
    "package_registry",
    "package_rollback",
    "package_uninstall",
    "service_package_launch",
    "service_package_verify",
    "settings_get",
    "settings_set",
    "surface_create",
    "surface_destroy",
    "surface_flip",
    "surface_write",
    "verify_package",
    "window_focus",
    "window_set_geometry",
    "window_set_input",
)

# A capability slot named in a comment or in a user-visible label is not
# a call, so the call patterns are matched against the code with both
# removed.
_COMMENT_OR_LITERAL = re.compile(
    r"//[^\n]*"
    r"|/\*.*?\*/"
    r'|"(?:[^"\\]|\\.)*"'
    r'|(?:b|br|r#*)"(?:[^"\\]|\\.)*"#*',
    re.DOTALL,
)


def code_only(source: str) -> str:
    """Return `source` with comments and string literals removed."""
    return _COMMENT_OR_LITERAL.sub(" ", source)


def slot_arguments(source: str) -> list[str]:
    """Return the SDK calls in `source` that still take a capability slot.

    A slot argument is either a bare number or the name of a resolved
    slot, and a call that names one is a call that pinned a position in
    the task's capability table.
    """
    code = code_only(source)
    return [
        name
        for name in SLOT_ARG_SDK_CALLS
        if re.search(
            r"\b" + name + r"\s*\(\s*(?:\d|[A-Za-z_][A-Za-z0-9_]*_slot\b|\bslot\b)",
            code,
        )
    ]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def vault_main() -> str:
    return read("apps/vault/src/main.rs")


def vault_source() -> str:
    """Everything the vault app is made of."""
    return "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((ROOT / "apps/vault/src").rglob("*.rs"))
    )


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
    """A capability slot is a position in a table, not an identity.

    WHY this contract changed: the kernel now authorises every platform
    call by the capability's *type*, and the SDK's functions no longer take
    a slot argument at all, so the app names the authority it wants -- a
    filesystem read -- and the kernel finds it. Pinning a slot number
    pinned an accident: the launcher proved it by passing slot 0 while its
    `FileSystemRead` capability sat at slot 3, so the developer-terminal
    flag it wanted was never read and the refusal reached the user with
    nothing on screen to say why. The old contract ("the read has to
    be resolved by its type") becomes "the app names no slot at all".
    """
    source = vault_source()
    assert slot_arguments(source) == []
    # The app resolves no slot: not through the SDK's slot helpers, and not
    # by looking a capability type up for itself.
    assert "std::cap::" not in source
    assert "std::cap_lookup(" not in source
    # The vault still reads the identity file it is given, and the file it
    # reads is the one it asked for.
    assert "std::fs_open(" in source
    assert "std::fs_read(" in source
    assert '"/system/etc/device_id"' in source


def test_the_display_capability_is_resolved_by_its_type():
    """The framebuffer capability is found by the kernel, not by the app.

    WHY this contract changed: a slot is a position in the task's
    capability table, and the position depends on the order the kernel
    inserted the capabilities in, so the app cannot name one. The kernel
    now authorises the surface by the capability's *type* and the SDK
    takes no slot argument, so there is nothing left for the app to look
    up. Pinning a slot number pinned an accident: the launcher proved it
    by passing slot 0 while its `FileSystemRead` capability sat at slot 3,
    so the developer-terminal flag it wanted was never read.
    """
    source = vault_source()
    assert slot_arguments(source) == []
    assert "std::cap::" not in source
    assert "std::cap_lookup(" not in source
    # The vault still opens a surface, presents it and tears it down.
    assert "std::surface_create(" in source
    assert "std::surface_flip(" in source
    assert "std::surface_destroy(" in source


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
