"""Truth contract for the InterOS sensor dashboard application.

A hardware inventory has one job: to say what the platform reports and
nothing else. Every assertion below pins a specific defect found by
reading the app.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def sensors_main() -> str:
    return read("apps/sensor_dashboard/src/main.rs")


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
    """The surface was created, flipped and destroyed through the literal
    slot 6."""
    source = sensors_main()
    assert "surface_create(6," not in source
    assert "surface_flip(6)" not in source
    assert "surface_destroy(6," not in source
    assert "std::cap::framebuffer_slot()" in source


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
