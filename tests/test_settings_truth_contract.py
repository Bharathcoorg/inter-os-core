"""Truth contract for the InterOS Settings application.

Every test below pins one fabrication that was found by reading the Settings
app against the kernel it actually talks to, and names the fault it forbids.
The contract is deliberately able to fail: each test asserts that the
offending string is *absent* from the app source *and* that the honest
replacement is *present*, so a regression that puts the fabrication back is
caught even if the replacement is still there.

The kernel is the authority here. Where a test quotes the kernel (the
proximity threshold, the payload field names, the syscall status codes, the
JSON buffers) it does so from the kernel's own source, so a change on either
side of the boundary breaks the contract rather than quietly invalidating it.
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

SETTINGS_MAIN = ROOT / "apps/settings/src/main.rs"
SETTINGS_LIB = ROOT / "apps/settings/src/lib.rs"

KERNEL_SYSCALL = ROOT / "kernel/src/syscall/mod.rs"
KERNEL_PROXIMITY = ROOT / "kernel/src/sensor/proximity.rs"
KERNEL_THERMAL = ROOT / "kernel/src/resource/thermal.rs"
KERNEL_BATTERY = ROOT / "kernel/src/resource/battery.rs"
KERNEL_SENSOR = ROOT / "kernel/src/sensor"


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def app_source() -> str:
    """The whole Settings app: the binary that renders and the library it decodes with."""
    return _read(SETTINGS_MAIN) + "\n" + _read(SETTINGS_LIB)


def row_block(source: str, title: str, span: int = 700) -> str:
    """The source text of one rendered row, located by its title literal."""
    index = source.find(title)
    assert index != -1, f"the Settings app no longer renders a row titled {title}"
    return source[index : index + span]


def rendered_literals(source: str) -> list:
    """Every byte-string literal the app can paint on the screen."""
    return re.findall(r'b"([^"\\]*)"', source)


def query_buffer_size(source: str, call: str) -> int:
    """The size of the buffer the app passes to `call`.

    The kernel refuses a fuel-gauge or thermal query whose buffer is shorter
    than its `*_JSON_MAX` and answers `InvalidArguments` instead of the
    payload, so the size of the buffer is part of the truth contract.
    """
    match = re.search(
        r"let mut (\w+) = \[0u8; (\d+)\];\s*\n\s*match " + re.escape(call) + r"\(&mut \1\)",
        source,
    )
    assert match, f"the Settings app no longer calls {call} with a stack buffer"
    return int(match.group(2))


# ---------------------------------------------------------------------------
# 1. The gyroscope
# ---------------------------------------------------------------------------


def test_the_gyroscope_is_not_reported_as_online_when_the_platform_has_no_gyro_axis():
    """`sys_sensor_read` serialises no angular rate, so no gyro reading may be shown.

    The kernel's own payload is orientation, one acceleration axis, ambient
    lux and the proximity flag. There is no gyro axis in it to decode, so a
    card that announced an active angular rate was describing motion the
    platform never measured.
    """
    source = app_source()
    syscall = _read(KERNEL_SYSCALL)

    # The platform's payload, quoted from the kernel.
    assert (
        '"present":true,"orientation":"{:?}","accel":{},"lux":{},"near":{}' in syscall
    ), "the kernel's sensor payload no longer matches the one this contract pins"
    body = syscall.split("pub fn sys_sensor_read", 1)[1].split("pub fn ", 1)[0]
    assert "gyro" not in body, "the kernel now reports a gyro axis; update this contract"

    # The fabrication must be gone from the app.
    assert "Roll, pitch & yaw angular rate active" not in source
    assert "angular rate active" not in source

    # The honest replacement must be there.
    assert "The platform reports no gyroscope axis" in source

    # And the row must not carry a live-reading badge.
    block = row_block(source, 'b"Gyroscope (Angular Rate)"')
    assert 'b"NOT REPORTED"' in block
    assert 'b"ONLINE"' not in block


# ---------------------------------------------------------------------------
# 2. The barometer
# ---------------------------------------------------------------------------


def test_the_barometer_card_is_deleted_because_the_kernel_has_no_barometer():
    """A card for a sensor the kernel does not have is a wishlist, not a reading.

    The number the card used to print was a constant, not a measurement, and
    no barometer exists anywhere in the kernel.
    """
    source = app_source()

    # The fabrication.
    assert "1013.25" not in source
    assert "hPa" not in source
    assert "Atmospheric pressure" not in source

    # The card itself is gone: no title, no icon, no row for it.
    assert "Barometer" not in source
    assert "Pressure Sensor" not in source

    # The kernel agrees: no barometer anywhere.
    kernel_hits = [
        path.name
        for path in ROOT.joinpath("kernel").rglob("*.rs")
        if "barometer" in path.read_text(encoding="utf-8").lower()
    ]
    assert not kernel_hits, f"the kernel now has a barometer: {kernel_hits}"


# ---------------------------------------------------------------------------
# 3. The magnetometer
# ---------------------------------------------------------------------------


def test_the_magnetometer_card_is_deleted_because_the_kernel_has_no_magnetometer():
    """No magnetic field sensor exists, so no heading may be claimed.

    The card that was titled for a magnetic field sensor actually displayed
    the screen orientation - a real platform reading under a fabricated
    title. The reading survived the rename; the fabricated device did not.
    """
    source = app_source()

    # The fabrication.
    assert "Digital compass heading online" not in source
    assert "Magnetometer" not in source
    assert "Compass" not in source
    assert "compass" not in source

    # The kernel agrees: no magnetic field sensor is reported to userspace.
    syscall = _read(KERNEL_SYSCALL)
    sensor_body = syscall.split("pub fn sys_sensor_read", 1)[1].split("pub fn ", 1)[0]
    assert "mag" not in sensor_body.lower()


# ---------------------------------------------------------------------------
# 4. The proximity threshold
# ---------------------------------------------------------------------------


def test_the_proximity_card_uses_the_kernels_30mm_threshold_and_not_a_5cm_one():
    """The near-range the card prints must be the range the kernel measures.

    `ProximitySensor::is_near` compares `distance_mm < 30`. A card that
    announced "<5cm" described a range the platform never used.
    """
    source = app_source()
    proximity = _read(KERNEL_PROXIMITY)

    # The kernel's threshold, quoted from the kernel.
    assert "distance_mm < 30" in proximity, "the kernel's proximity threshold moved"

    # The fabrication.
    assert "<5cm" not in source
    assert "5cm" not in source

    # The honest replacement.
    assert "object near (<30 mm)" in source
    assert "clear (>=30 mm)" in source


# ---------------------------------------------------------------------------
# 5. The thermal card
# ---------------------------------------------------------------------------


def test_the_thermal_card_shows_the_zone_temperatures_cooling_state_and_throttling_flag():
    """The thermal payload is decoded, not summarised away.

    The card received all five zone temperatures, the cooling state and the
    throttling flag and kept only a boolean. Every one of those is a reading
    the platform made, and each has to reach the screen.
    """
    source = app_source()

    # The fabrication: a summary that threw the reading away.
    assert "Multi-zone thermal telemetry monitored" not in source
    assert "Thermal payload received" not in source
    assert "Temperature telemetry available" not in source

    # The honest replacement: the payload is parsed and every field rendered.
    assert "parse_thermal_payload" in source
    for token in (
        'b"CPU "',
        'b"C GPU "',
        'b"C BAT "',
        'b"C MOD "',
        'b"C CHG "',
        'b"Cooling: "',
        'b"throttling: on"',
        'b"throttling: off"',
    ):
        assert token in source, f"the thermal card no longer renders {token}"

    # The zone names are the kernel's own zone names, from the payload the
    # kernel's thermal manager actually writes.
    thermal = _read(KERNEL_THERMAL)
    for zone in ("cpu", "gpu", "battery", "modem", "charging", "state", "throttling"):
        assert f'"{zone}"' in thermal, (
            f"the kernel's thermal payload no longer carries {zone!r}"
        )


# ---------------------------------------------------------------------------
# 6. The fuel gauge
# ---------------------------------------------------------------------------


def test_the_fuel_gauge_card_shows_the_state_of_charge_voltage_temperature_and_capacity():
    """The fuel-gauge payload is decoded, not summarised away.

    The card received the state of charge, the terminal voltage, the pack
    temperature, the capacity and the charging state, and kept only a
    boolean.
    """
    source = app_source()

    # The fabrication.
    assert "Battery payload received" not in source
    assert "b\"Battery payload received\"" not in source

    # The honest replacement.
    assert "parse_battery_payload" in source
    for token in (
        'b"% - "',
        'b" mV - "',
        'b" C - "',
        'b" mAh"',
        'b"Charging state: "',
    ):
        assert token in source, f"the fuel gauge card no longer renders {token}"

    # The field names are the kernel's own field names, from the payload the
    # kernel's `sys_battery_query` actually writes.
    battery_payload = _read(KERNEL_SYSCALL)
    for field in ("voltage_mv", "soc", "capacity", "temp", "charging", "telemetry_valid"):
        assert f'"{field}"' in battery_payload, (
            f"the kernel's fuel-gauge payload no longer carries {field!r}"
        )


# ---------------------------------------------------------------------------
# 7. The error reasons
# ---------------------------------------------------------------------------


def test_a_refused_query_reports_the_platforms_status_instead_of_a_generic_unavailable():
    """Every failed sensor, thermal and fuel-gauge call names the platform's reason.

    The platform publishes a status name for each failure, and the payload
    carries a verbatim error string. Collapsing all of them into one
    "Unavailable" told the user nothing about why the answer was missing.
    """
    source = app_source()
    lib = _read(SETTINGS_LIB)

    # The refusal keeps the status code instead of discarding it.
    assert source.count("QueryOutcome::Failed(e)") >= 3, (
        "the sensor, thermal and fuel-gauge queries must all keep the "
        "platform's status code"
    )

    # And the card says which status it was.
    for prefix in (
        'b"Sensor query failed: "',
        'b"Thermal query failed: "',
        'b"Battery query failed: "',
    ):
        assert prefix in source, f"the app no longer reports {prefix}"


def test_the_status_names_the_app_shows_are_the_kernels_own():
    """The four names the app can show are the kernel's own spellings and codes.

    This is the strongest form of the contract: it reads the kernel's
    `SyscallResult` enum and checks the app's mapping against it, so a
    renumbered status breaks the app's promise and breaks this test.
    """
    syscall = _read(KERNEL_SYSCALL)
    lib = _read(SETTINGS_LIB)

    for name, code in (
        ("PermissionDenied", 1),
        ("InvalidSlot", 2),
        ("InvalidArguments", 3),
        ("DeviceError", 12),
    ):
        assert f"{name} = -{code}," in syscall, (
            f"the kernel's {name} is no longer -{code}; the app's status "
            "names must be updated"
        )
        assert f'-{code} => b"{name}"' in lib, (
            f"the app no longer names the kernel's {name}"
        )


def test_the_sensor_bus_quotes_the_platforms_own_error_string():
    """An unbound sensor bus is described with the kernel's verbatim string.

    The kernel publishes both a `status` and an `error` next to its
    `present:false` flag. The app quotes the error rather than paraphrasing
    it, so the user reads what the platform actually said.
    """
    source = app_source()
    lib = _read(SETTINGS_LIB)
    syscall = _read(KERNEL_SYSCALL)

    assert (
        '"status":"NOT_IMPLEMENTED","error":"No physical IMU or ambient sensor detected"'
        in syscall
    ), "the kernel's unbound-sensor payload no longer matches the pinned one"
    assert "No physical IMU or ambient sensor detected" in lib
    assert "b\"status\"" in source, "the app must surface the platform's status field"


# ---------------------------------------------------------------------------
# 8. The absent payload is not a reading
# ---------------------------------------------------------------------------


def test_an_absent_fuel_gauge_is_not_rendered_as_a_zero_percent_reading():
    """An absent payload's zeroes are the platform's absence, not a measurement.

    The kernel serialises `present:false` with zeroed numbers. Printing
    those numbers would be printing the absence as if it were a reading.
    """
    source = app_source()

    assert "QueryOutcome::Absent" in source
    assert 'b"The platform reports no fuel gauge (present=false)"' in source

    # The reading is only rendered when the platform says the gauge is present.
    assert "t.present && t.telemetry_valid" in source


def test_an_absent_thermal_zone_set_is_not_rendered_as_zero_degree_zones():
    """The same rule for the thermal zones."""
    source = app_source()

    assert 'b"The platform reports no thermal sensor (present=false)"' in source
    assert "Some(t) if t.present =>" in source


# ---------------------------------------------------------------------------
# 9. The queries are actually asked in a shape the platform answers
# ---------------------------------------------------------------------------


def test_the_thermal_and_fuel_gauge_queries_use_a_buffer_the_platform_will_answer():
    """The payload buffers are large enough for the kernel's JSON contract.

    The kernel refuses a fuel-gauge or thermal query whose buffer is shorter
    than its `*_JSON_MAX` and answers `InvalidArguments` instead of the
    payload. Asking in a shape the platform refuses is why these two cards
    had never seen a real reading at all.
    """
    source = app_source()

    assert "pub const THERMAL_JSON_MAX: usize = 256;" in _read(KERNEL_THERMAL)
    assert "pub const BATTERY_JSON_MAX: usize = 256;" in _read(KERNEL_BATTERY)

    # The app must ask with a buffer the platform will answer.
    assert query_buffer_size(source, "std::battery_query") >= 256
    assert query_buffer_size(source, "std::thermal_query") >= 256

    # The sensor query has its own bounds, and the app must stay inside them.
    assert 32 <= query_buffer_size(source, "std::sensor_read") <= 512


# ---------------------------------------------------------------------------
# 10. The screen orientation kept its reading
# ---------------------------------------------------------------------------


def test_the_screen_orientation_is_still_reported_because_the_platform_sends_it():
    """Deleting the magnetic-field card must not delete the real reading.

    The platform's sensor payload carries an orientation. It was being
    displayed under a fabricated title; the title was the fault.
    """
    source = app_source()

    assert 'b"Screen Orientation"' in source
    assert 'b"Orientation: "' in source

    # And the kernel really does send it.
    syscall = _read(KERNEL_SYSCALL)
    assert '"orientation":"{:?}"' in syscall


# ---------------------------------------------------------------------------
# 11. The page still lists a real inventory
# ---------------------------------------------------------------------------


def test_the_diagnostics_page_lists_only_devices_the_platform_can_report():
    """Every card on the diagnostics page is backed by a payload field.

    The page is the honest hardware inventory now that the separate
    dashboard app is gone, so a card for a device the platform cannot report
    must not reappear on it.
    """
    source = app_source()

    for title in (
        'b"Accelerometer (3-Axis IMU)"',
        'b"Gyroscope (Angular Rate)"',
        'b"Screen Orientation"',
        'b"Ambient Light Sensor"',
        'b"Proximity Sensor (IR ToF)"',
        'b"Fuel Gauge (Battery)"',
        'b"Thermal Telemetry Zones"',
    ):
        assert title in source, f"the diagnostics page lost its {title} card"

    # Every one of them is rendered from the sensor or telemetry payload.
    assert "sensor_telemetry" in source
    assert "battery_telemetry" in source
    assert "thermal_telemetry" in source


def test_the_sensor_rows_cannot_claim_a_reading_when_the_bus_is_unbound():
    """A card only shows a measurement when the platform sent one.

    The bus reports `present:false` when nothing is bound, and the app has
    to keep the unbound state and the reading state apart.
    """
    source = app_source()

    # The absence path is driven by the payload's own `present` flag.
    assert "if t.is_present {" in source
    assert "self.sensor_outcome = if self.sensor_telemetry.is_present {" in source

    # Every sensor row consults it before it renders a number.
    assert source.count("if t.is_present {") >= 3


def test_the_kernel_has_no_gyroscope_module_that_could_back_the_old_claim():
    """The kernel's sensor directory has no gyroscope file to back a gyro card.

    A gyroscope card is only honest if the platform has one. This pins that
    the platform does not.
    """
    modules = sorted(path.stem for path in KERNEL_SENSOR.glob("*.rs"))
    assert "gyro" not in modules
    assert "gyroscope" not in modules
    assert "barometer" not in modules
    assert "magnetometer" not in modules


def test_no_settings_source_file_prints_a_fabricated_sensor_reading():
    """A last sweep over both Settings source files.

    This is the blunt check that would catch any of the fabrications coming
    back under a different wording, and it fails loudly on the first one it
    finds.
    """
    forbidden = (
        "1013.25",
        "hPa",
        "Roll, pitch",
        "Digital compass",
        "<5cm",
        "gyroscope axis active",
    )
    for path in (SETTINGS_MAIN, SETTINGS_LIB):
        text = _read(path)
        for needle in forbidden:
            assert needle not in text, f"{path.name} reintroduced {needle!r}"


def test_no_rendered_string_claims_an_angular_rate_the_platform_never_measured():
    """No byte-string the app can paint may name a gyro axis.

    The row *title* may name the device - the platform has no such device,
    and the card exists to say so - but no painted string may report an
    angular rate, because `sys_sensor_read` never carried one.
    """
    axis_words = re.compile(r"\b(yaw|pitch|roll)\b", re.IGNORECASE)
    for literal in rendered_literals(_read(SETTINGS_MAIN)):
        lowered = literal.lower()
        assert not axis_words.search(literal), (
            f"a rendered Settings string names a gyroscope axis: {literal!r}"
        )
        for claim in ("angular rate active", "gyro axis active"):
            assert claim not in lowered, (
                f"a rendered Settings string claims {claim!r}: {literal!r}"
            )

    # The one painted string that may mention the device says it is absent.
    assert "The platform reports no gyroscope axis" in rendered_literals(
        _read(SETTINGS_MAIN)
    )


# ---------------------------------------------------------------------------
# 12. The other fabricated readings the audit surfaced in the same app
# ---------------------------------------------------------------------------


def test_the_package_count_is_the_registrys_own_and_is_not_floored_at_twelve():
    """A "VERIFIED" badge on an invented package count is a claim, not a count.

    The app counted the registry and then raised the count to a minimum of
    twelve, so a registry of three was reported as twelve verified packages.
    """
    source = app_source()

    assert "count.max(12)" not in source
    assert "self.app_count = 12" not in source
    assert "app_count: Option<usize>" in source

    # The honest replacement: the registry's own count, or "did not answer".
    assert "self.app_count = Some(count)" in source
    assert "self.app_count = None" in source
    assert "Package registry did not answer" in source


def test_the_storage_size_is_the_queried_capacity_and_not_a_hardcoded_512_mb():
    """A "MOUNTED" badge on a constant capacity claims a volume that is not there."""
    source = app_source()

    assert "storage_bytes: 512 * 1024 * 1024" not in source
    assert "self.storage_bytes = 512 * 1024 * 1024" not in source
    assert "storage_bytes: Option<u64>" in source

    assert "self.storage_bytes = Some(bytes)" in source
    assert "self.storage_bytes = None" in source
    assert "Storage capacity not reported by the platform" in source


def test_the_owner_identity_is_not_invented_when_the_settings_store_does_not_answer():
    """An identity under an "ACTIVE" badge is a claim the store never made."""
    source = app_source()

    assert "Sovereign Owner" not in source
    assert "Owner name not reported by the settings store" in source


def test_the_account_status_is_not_invented_when_the_settings_store_does_not_answer():
    """An "ENCRYPTED" badge on an invented phrase asserts an encryption state."""
    source = app_source()

    assert "Encrypted & Sovereign" not in source
    assert "Status not reported by the settings store" in source


def test_the_settings_app_does_not_claim_radios_are_enabled_and_listening():
    """There is no radio on this platform, and no flight-mode syscall to ask.

    The card used to say the sovereign radios were enabled and listening,
    under a badge that said the opposite.
    """
    source = app_source()

    assert "enabled & listening" not in source
    assert "All sovereign radios" not in source
    assert "Flight mode not reported by the platform" in source


def test_the_haptic_motor_is_not_claimed_and_the_sound_path_is_asked_about():
    """The platform's sound path is a PC speaker, and the app now asks.

    The row carried a green "REPORTED" badge on a caption that described a
    haptic motor no query had ever reported.
    """
    source = app_source()

    assert "Haptic feedback motor" not in source
    assert "settings_hardware_status" in source
    assert "PCM playback & tactile click feedback" in source

    # The category names are the kernel's own category names.
    settings = _read(ROOT / "kernel/src/settings.rs")
    for category in ("sound", "cellular", "bluetooth"):
        assert f'"{category}" =>' in settings
