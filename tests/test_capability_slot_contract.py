"""Contract tests for how an app addresses a capability.

A capability slot is a position in a table. The position depends on the
order the task's capabilities were inserted in, so the same number means
different capabilities in different apps, and a number that is right in
one app is wrong in the next.

The platform now authorises every call by the capability's *type*, and the
SDK's functions no longer take a slot. These tests pin that: the slot
argument is gone from the ABI's callers, the kernel does not verify a
caller-named slot, and no app carries a slot of its own.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
APPS = ROOT / "apps"
USERSPACE = ROOT / "userspace"

# The platform calls that used to take a capability slot as their first
# argument. They do not any more, and these tests pin that.
#
# WHY the list is complete rather than a sample: a contract that checks
# fourteen of the thirty-one calls lets the other seventeen drift back
# without anything failing.
SLOT_CALLS = (
    "app_storage_open",
    "display_set_mode",
    "display_set_orientation",
    "fs_list",
    "fs_open",
    "fs_read",
    "fs_read_at",
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

# Apps that have not moved to the typed helpers yet.
#
# WHY this list exists rather than a blanket failure: the list makes the
# remaining work visible and it shrinks as each app is migrated. Removing
# an entry is a change to this file, so the debt cannot grow unnoticed.
PENDING: set[str] = set()


# The names an app used to give a capability slot of its own. A call that
# takes one of these is passing a position.
#
# WHY the check is on the name and not on "a number": the SDK's platform
# calls no longer take a slot, and several of them legitimately take a
# number first -- a surface's width, a window's x, a file descriptor. A
# number is no longer evidence of a slot; a variable named after one is.
SLOT_NAMES = (
    "cap_slot",
    "display_cap_slot",
    "fb_cap",
    "fb_slot",
    "fs_cap",
    "fs_cap_slot",
    "fs_slot",
    "ipc_cap_slot",
    "ipc_slot",
    "log_slot",
    "package_slot",
    "r_slot",
    "read_slot",
    "settings_slot",
    "STORE_CAP_READ",
    "STORE_CAP_WRITE",
    "t_slot",
    "tick_slot",
    "w_slot",
    "write_slot",
)


def _app_roots():
    for root in (APPS, USERSPACE):
        if root.is_dir():
            for directory in sorted(root.iterdir()):
                if directory.is_dir():
                    yield directory


def _offenders(app_dir: Path) -> list[str]:
    """Every call that passes a capability slot of the app's own."""
    found: list[str] = []
    for source in sorted(app_dir.rglob("*.rs")):
        text = source.read_text(encoding="utf-8")
        for call in SLOT_CALLS:
            for match in re.finditer(
                rf"\b{re.escape(call)}\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)", text
            ):
                if match.group(1) in SLOT_NAMES:
                    found.append(
                        f"{source.relative_to(ROOT)}:{call}({match.group(1)})"
                    )
        # A resolved slot is allowed to answer "do I hold this kind of
        # capability?" and nothing else. It must never be bound and carried.
        for match in re.finditer(
            r"std::cap(?:::cap_lookup|_lookup|\w*_slot)\(", text
        ):
            window = text[match.end() : match.end() + 160]
            if not any(
                marker in window
                for marker in (
                    ".is_ok()",
                    ".is_err()",
                    ".is_none()",
                    ".is_some()",
                    ".ok()",
                    "if let Some(_)",
                )
            ):
                found.append(f"{source.relative_to(ROOT)}:resolved slot is carried")
    return found


def test_no_app_addresses_a_capability_by_its_position():
    """Every capability-taking call resolves its slot by type.

    WHY this matters: the slot a capability occupies is decided by the
    order the kernel inserted that app's capabilities in. The vault, for
    instance, read its files through the slot that held its write
    capability and called the keystore through the slots that held the
    serial port and the IPC channel. Every one of those calls was
    refused, and the app reported the refusal as a hardware fact.
    """
    failures = []
    for app_dir in _app_roots():
        if not app_dir.is_dir() or app_dir.name in PENDING:
            continue
        offenders = _offenders(app_dir)
        if offenders:
            failures.append(
                f"{app_dir.name}: " + ", ".join(sorted(set(offenders)))
            )
    assert not failures, (
        "an app addresses a capability by its position rather than by its "
        "type:\n  " + "\n  ".join(failures)
    )


def test_the_pending_list_names_only_apps_that_exist():
    """The exception list must not name an app that is not there.

    WHY: a stale entry would let an app drift back to a hardcoded slot
    without the list ever failing.
    """
    for name in sorted(PENDING):
        assert (APPS / name).is_dir(), f"the pending list names {name}, which is not an app"


def test_the_sdk_offers_a_typed_slot_for_every_capability_an_app_needs():
    """The helpers the migration needs must exist and resolve.

    WHY: the migration is only fair if the replacement is available.
    Without this, an app that must stop hardcoding a slot has nothing to
    call instead.
    """
    sdk = (ROOT / "sdk" / "rust" / "interos-std" / "src" / "lib.rs").read_text(
        encoding="utf-8"
    )
    for helper in (
        "pub fn fs_read_slot()",
        "pub fn fs_write_slot()",
        "pub fn framebuffer_slot()",
        "pub fn timer_slot()",
        "pub fn event_poll_slot()",
        "pub fn network_socket_slot()",
        "pub fn network_send_slot()",
        "pub fn network_receive_slot()",
        "pub fn rtc_slot()",
    ):
        assert helper in sdk, f"the SDK does not offer {helper}"


def test_the_sdk_does_not_offer_a_call_that_takes_a_capability_slot():
    """The SDK's platform calls take the authority, not a position.

    WHY this matters: a slot-taking call in the SDK is an invitation for the
    next app to pass a number, and the number will be wrong the moment the
    policy gains or loses a capability. The kernel authorises by type, so
    the slot has no meaning to pass.
    """
    sdk = (ROOT / "sdk" / "rust" / "interos-std" / "src" / "lib.rs").read_text(
        encoding="utf-8"
    )
    for call in SLOT_CALLS:
        signature = re.search(rf"pub fn {re.escape(call)}\s*\(([^)]*)\)", sdk)
        assert signature is not None, f"the SDK does not offer {call}"
        parameters = signature.group(1)
        assert "cap_slot" not in parameters, (
            f"{call} still takes a capability slot: {parameters}"
        )


def test_the_kernel_does_not_verify_a_caller_named_slot():
    """The kernel authorises by capability type.

    WHY this matters: verifying whatever slot the caller named makes every
    caller responsible for its own capability-table layout. The launcher
    proved the point -- it passed slot 0 while its FileSystemRead
    capability sat at slot 3, and the call was refused with nothing on
    screen to say why.
    """
    syscall = (ROOT / "kernel" / "src" / "syscall" / "mod.rs").read_text(
        encoding="utf-8"
    )
    vfs = (ROOT / "kernel" / "src" / "fs" / "vfs.rs").read_text(encoding="utf-8")
    for source, name in ((syscall, "syscall/mod.rs"), (vfs, "fs/vfs.rs")):
        offenders = re.findall(r"\.verify\(\s*(?:cap_slot|eff_slot)\b", source)
        assert not offenders, (
            f"{name} still verifies the slot the caller named: {offenders}"
        )


def test_the_kernel_finds_a_capability_by_its_type():
    """The primitive the migration relies on has to exist and be used.

    WHY: `authorize_capability` walks the table and matches on the
    capability's type and rights, so a caller that names the authority it
    wants gets it whether it sits in slot 0 or slot 7.
    """
    capability = (ROOT / "kernel" / "src" / "capability" / "mod.rs").read_text(
        encoding="utf-8"
    )
    assert "pub fn authorize_capability(" in capability
    assert "pub fn find_capability(" in capability
    assert "pub fn find_capability_for_object(" in capability
    syscall = (ROOT / "kernel" / "src" / "syscall" / "mod.rs").read_text(
        encoding="utf-8"
    )
    assert syscall.count("authorize_capability(") >= 10, (
        "the syscall layer should authorise by capability type"
    )
