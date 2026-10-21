"""Truth contract for the InterOS sensor dashboard application.

A hardware inventory has one job: to say what the platform reports and
nothing else. Every assertion below pins a specific defect found by
reading the app.
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


def sensors_main() -> str:
    return read("apps/sensor_dashboard/src/main.rs")


def sensors_source() -> str:
    """Everything the sensor dashboard app is made of."""
    return "\n".join(
        path.read_text(encoding="utf-8")
        for path in sorted((ROOT / "apps/sensor_dashboard/src").rglob("*.rs"))
    )


def test_a_refused_read_is_not_reported_as_absent_hardware():
    """The inventory said the hardware flag was false for every failure.

    A refused call never reached the hardware check, so that was a claim
    the app could not support.
    """
    source = sensors_main()
    assert "hardware_present is false here" not in source
    assert "Telemetry::Denied(_) => b\"sensor_read was refused\"" in source
    assert "payload lacked the expected fields" in source


def test_the_orientation_row_does_not_contradict_the_bus_card():
    """The bus card printed the orientation the payload carried.

    The inventory row below it said "unavailable" for the same channel
    whatever the payload said.
    """
    source = sensors_main()
    assert 'channel!(4, b"Screen orientation", b"measured", OK, b"sensor_read() -> orientation");' in source
    assert 'channel!(4, b"Screen orientation", b"unavailable", WARN, b"derived from accel; none present");' in source


def test_the_closing_sentence_is_built_from_the_two_answers():
    """It used to be a fixed line saying the inventory was simulated and
    the bus had no hardware, whatever either had answered."""
    source = sensors_main()
    assert "Inventory says 'simulated'; the bus says no hardware." not in source
    assert 'b"the bus reports hardware"' in source
    assert 'b"the bus says no hardware"' in source
    assert "Inventory was refused; " in source


def test_an_acceleration_reading_carries_its_unit():
    """The kernel's IMU reports acceleration in milli-g.

    A bare number is a measurement without its scale.
    """
    source = sensors_main()
    assert 'b" mg"' in source
    assert "sensor_read() -> accel (milli-g)" in source


def test_a_capability_slot_is_resolved_by_its_type():
    """A capability slot is a position in a table, not an identity.

    WHY this contract changed: a slot is a position in the task's
    capability table, and the position depends on the order the kernel
    inserted the capabilities in, so the app cannot name one. The kernel
    now authorises every platform call by the capability's *type* and the
    SDK's functions no longer take a slot argument, so there is nothing
    left for the app to resolve. Pinning a slot number pinned an
    accident: the launcher proved it by passing slot 0 while its
    `FileSystemRead` capability sat at slot 3, so the developer-terminal
    flag it wanted was never read and the refusal reached the user with
    nothing on screen to say why.
    """
    source = sensors_source()
    assert slot_arguments(source) == []
    # The app resolves no slot: not through the SDK's slot helpers, and not
    # by looking a capability type up for itself.
    assert "std::cap::" not in source
    assert "std::cap_lookup(" not in source
    # The dashboard still opens a surface, presents it and tears it down.
    assert "std::surface_create(" in source
    assert "std::surface_flip(" in source
    assert "std::surface_destroy(" in source


def test_the_app_ships_and_is_registered_but_is_not_required_to_boot():
    """The crate ships, the launcher registers it, and it is deliberately
    absent from the boot manifest.

    A hardware inventory is not something the device needs in order to
    boot, so it is registered and launchable without being required. The
    documentation in the app ecosystem repository described the app as
    removed, which contradicted all three of these facts.
    """
    manifest = read("platform/x86_64/userspace-manifest.json")
    assert "org.interos.sensor_dashboard" not in manifest
    launcher = read("apps/launcher/src/main.rs")
    assert "org.interos.sensor_dashboard" in launcher
    # The crate itself is in the tree, and it is the one the boot image
    # would build if the manifest ever listed it.
    assert (ROOT / "apps/sensor_dashboard/src/main.rs").is_file()
    assert (ROOT / "apps/sensor_dashboard/src/lib.rs").is_file()
