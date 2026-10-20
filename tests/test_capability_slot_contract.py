"""Contract tests for how an app addresses a capability.

A capability slot is a position in a table. The position depends on the
order the task's capabilities were inserted in, so the same number means
different capabilities in different apps, and a number that is right in
one app is wrong in the next.

These tests pin the rule that an app asks for a capability by its type
and never by its position.
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
APPS = ROOT / "apps"

# The syscalls that take a capability slot as their first argument.
SLOT_CALLS = (
    "fs_open",
    "fs_read",
    "fs_write",
    "fs_list",
    "settings_get",
    "settings_set",
    "get_ticks",
    "surface_create",
    "surface_flip",
    "surface_destroy",
    "keystore_derive",
    "app_storage_open",
    "ipc_send",
    "ipc_recv",
)

# Apps that have not moved to the typed helpers yet.
#
# WHY this list exists rather than a blanket failure: the list makes the
# remaining work visible and it shrinks as each app is migrated. Removing
# an entry is a change to this file, so the debt cannot grow unnoticed.
#
# The apps here are being worked by another agent. The slot numbers in
# them happen to be correct today, because every app's capability list
# happens to place the framebuffer at the same index -- which is exactly
# the accident that made this worth pinning.
PENDING = {
    "dialer",
    "file_manager",
    "gallery",
    "launcher",
    "messages",
    "music",
    "sensor_dashboard",
    "settings",
    "terminal",
    "vault",
}


def _offenders(app_dir: Path) -> list[str]:
    """Every call that passes a number where a capability slot belongs."""
    found: list[str] = []
    for source in sorted(app_dir.rglob("*.rs")):
        text = source.read_text(encoding="utf-8")
        for call in SLOT_CALLS:
            for match in re.finditer(rf"\b{re.escape(call)}\s*\(\s*(\d+)\b", text):
                found.append(f"{source.relative_to(ROOT)}:{call}({match.group(1)})")
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
    for app_dir in sorted(APPS.iterdir()):
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
