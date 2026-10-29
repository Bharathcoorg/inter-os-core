"""Platform Truth Contract: Status Band Adoption and Service State Backing.

Every top-of-screen status band rendered by an application must:
1. Use the canonical platform status band component (StatusBand / StatusBar) at y < STATUS_BAR_HEIGHT (48 px).
2. Present real service state backed by hardware queries:
   - Clock must come from `std::rtc::Reading::now()` queried once per frame/event tick,
     never per-pixel.
   - When the RTC is unavailable, clock must be None - never an optimistic default (00:00).
   - Telemetry marks (cellular, wifi, battery) must be outlines (unbound) unless backed
     by a real hardware/service query (e.g., dialer telephony/battery query, settings battery query).
3. Never render state with no backing query.
"""

from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
APPS_DIR = ROOT / "apps"

TARGET_APPS = [
    "calculator",
    "calendar",
    "camera",
    "dialer",
    "gallery",
    "messages",
    "music",
    "notes",
    "settings",
    "vault",
]


def read_source(subpath: str) -> str:
    return (ROOT / subpath).read_text(encoding="utf-8")


def code_only(source: str) -> str:
    """Strips comments so checks are strictly against executable code."""
    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def test_sdk_exports_status_band():
    """Verify that interos-std design components export StatusBand and status_band()."""
    status_bar_rs = read_source("sdk/rust/interos-std/src/design/components/status_bar.rs")
    code = code_only(status_bar_rs)
    assert "pub type StatusBand = StatusBar;" in code, (
        "interos-std must export StatusBand as an alias for StatusBar"
    )
    assert "fn status_band()" in code, (
        "interos-std must export status_band() extent helper"
    )


def test_status_band_behavior_and_pixels_pass():
    """Execute real status bar unit tests in interos-std to verify pixel and telemetry behavior.

    Platform Truth Contract:
    - Zero inked pixels when instruments are unsupported
    - Unbound instruments draw outlines only, never filled levels
    - Clock lane draws zero inked pixels when RTC is unavailable (None)
    - Fuel gauge and radio bars draw solid pixels only when backed by measured readings
    - Status band paints into real Surface and divides itself with border line
    - Unbound strip inks exactly 214 pixels (canonical shell geometry)
    """
    res = subprocess.run(
        ["cargo", "test", "--manifest-path", "sdk/rust/interos-std/Cargo.toml", "status_bar"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert res.returncode == 0, f"status_bar unit tests failed:\n{res.stdout}\n{res.stderr}"
    assert "10 passed" in res.stdout, f"Expected 10 status_bar tests to pass, got:\n{res.stdout}"


def test_all_target_apps_adopt_status_band():
    """Every target app must adopt the platform status band (StatusBand / StatusBar)."""
    for app_name in TARGET_APPS:
        main_path = f"apps/{app_name}/src/main.rs"
        source = read_source(main_path)
        code = code_only(source)

        assert (
            "StatusBand" in code
            or "StatusBar" in code
            or "status_band" in code
        ), f"app '{app_name}' does not adopt StatusBand / StatusBar in {main_path}"

        # Must check the top band bounds: either STATUS_BAR_HEIGHT or 48 or HEIGHT
        assert (
            "STATUS_BAR_HEIGHT" in code
            or "STATUS_H" in code
            or "status_band" in code
            or "y < 48" in code
        ), f"app '{app_name}' does not guard top status band inset"


def test_band_renders_no_state_without_backing_query():
    """A band must not render state (clock, battery, signal) without a real backing query.

    Platform Truth Contract:
    - Clock must be backed by std::rtc::Reading::now().
    - Clock must NOT be read inside the per-pixel inner loop.
    - No fabricated clock literals (e.g. "09:20", "12:00", "00:00").
    - Unavailable state must remain None / Unbound, never optimistic defaults.
    """
    for app_name in TARGET_APPS:
        main_path = f"apps/{app_name}/src/main.rs"
        source = read_source(main_path)
        code = code_only(source)

        # Backing query check: clock reading must come from RTC
        assert "rtc::Reading::now()" in code, (
            f"app '{app_name}' renders status band without backing clock query (rtc::Reading::now())"
        )

        # No fabricated clock strings
        assert 'b"09:20"' not in code, f"app '{app_name}' contains fabricated clock 09:20"
        assert 'b"12:00"' not in code, f"app '{app_name}' contains fabricated clock 12:00"

        # The clock must be read once per frame/tick, not inside per-pixel loops
        for fn_name in [
            f"get_{app_name}_pixel",
            "render_pixel",
            "header_pixel",
            "landing_pixel",
        ]:
            if f"fn {fn_name}" in code:
                # Find function body
                start = code.index(f"fn {fn_name}")
                # Approximate function extent up to 3000 chars or next fn
                next_fn = code.find("\nfn ", start + 1)
                end = next_fn if next_fn != -1 else start + 3000
                fn_body = code[start:end]
                assert "rtc::Reading::now()" not in fn_body, (
                    f"app '{app_name}' reads RTC inside per-pixel function {fn_name}; "
                    "must be read once per frame/tick outside pixel rasterization"
                )


def test_every_app_treats_unavailable_as_unavailable():
    """When RTC reading fails (None), no clock text is rendered (clock_ok = false)."""
    for app_name in TARGET_APPS:
        main_path = f"apps/{app_name}/src/main.rs"
        source = read_source(main_path)
        code = code_only(source)

        assert "clock_ok" in code or "clock.is_some()" in code or "Some(&" in code, (
            f"app '{app_name}' must explicitly check clock availability before rendering clock lane"
        )


def test_cellular_mark_requires_measured_level_not_modem_presence():
    """Platform Truth Contract: a filled cellular mark is a measured level.

    The messages band used to answer `Reading::Measured(100)` from the
    `modem_present` boolean alone, so a modem merely being attached drew four
    full bars -- a level no instrument took. A cellular mark may be `Measured`
    only where the telephony payload's own `"bars"` count drove it, the way the
    dialer already does it; presence on its own has to leave the mark unbound.
    """
    messages = code_only(read_source("apps/messages/src/main.rs"))

    # The dishonest shape must be gone: no full-strength mark keyed off presence.
    assert "Reading::Measured(100)" not in messages, (
        "messages must not draw a cellular mark of 100 that no instrument measured"
    )
    cellular_start = messages.index("let cellular =")
    cellular_lane = messages[cellular_start: messages.index("let instruments", cellular_start)]
    assert "modem_present" not in cellular_lane, (
        "messages' cellular mark must not be gated on modem presence alone"
    )
    assert "signal_bars" in cellular_lane, (
        "messages' cellular mark must be driven by the measured bar count"
    )

    # The measured mark comes from the baseband's own bar count.
    assert '\\"bars\\"' in messages and "json_u8(payload," in messages, (
        "messages must read the measured bar count out of the telephony payload"
    )

    # No reading stays an explicit unbound outline.
    assert "Reading::Unbound" in messages, (
        "messages must report an unbound cellular mark when the radio gave no level"
    )


def test_file_manager_adopts_band_with_only_obtainable_instruments():
    """file_manager draws the platform band and asks the platform for its clock.

    The band is the shell's chrome, so the file manager takes the top inset from
    the platform rather than its own header; and because the app obtains no
    radio, Wi-Fi or fuel-gauge reading, its instruments stay unbound -- the
    storage figure it does measure lives on its own storage card, where it has
    always been.
    """
    fm = code_only(read_source("apps/file_manager/src/main.rs"))

    assert "components::StatusBand::new(W)" in fm, "file_manager must adopt the platform band"
    assert "y < STATUS_BAR_HEIGHT" in fm, "file_manager must reserve the platform's top inset"
    assert "std::design::viewport::STATUS_BAR_HEIGHT" in fm, (
        "file_manager must take the inset from the platform, not retype it"
    )
    assert "std::rtc::Reading::now()" in fm, (
        "file_manager's clock lane must be backed by the RTC"
    )
    assert "components::Instruments::unmeasured()" in fm, (
        "file_manager has no radio/fuel instrument, so its band marks stay unbound"
    )
    # The clock is read on the loop's tick, never inside the rasteriser.
    band_fn = fm[fm.index("fn get_file_manager_color"): fm.index("fn refresh_clock")]
    assert "rtc::Reading::now()" not in band_fn, (
        "file_manager must not read the RTC per pixel"
    )
    # Its honest storage line stays.
    assert "storage_status" in fm, "file_manager must keep its measured storage line"


def _kernel_spawn_bodies() -> dict:
    """Each first-party app's capability table as the kernel builds it."""
    src = read_source("kernel/src/task/user.rs")
    return {
        match.group(1).removeprefix("spawn_app_"): match.group(2)
        for match in re.finditer(
            r"pub fn (spawn_app_\w+)\(id_security: TaskId\) -> TaskId \{(.*?)\n\}\n",
            src,
            flags=re.S,
        )
    }


def test_an_app_that_draws_the_band_clock_lane_holds_the_clock_read():
    """The band's clock lane and the kernel's authority for it are one decision.

    WHY this crosses the app/kernel boundary: an app that calls
    `rtc::Reading::now()` without `RealtimeClockRead` is refused by the syscall,
    and by contract it must draw a blank lane rather than an invented time. So a
    band that reaches for the clock on an app the kernel never authorised looks
    correct in the app's own source and is wrong on the device -- the drift this
    pins is exactly the one where the rendering half lands and the authority
    half is forgotten.
    """
    spawns = _kernel_spawn_bodies()
    assert spawns, "no app spawn tables found in the kernel -- the scan tests nothing"

    checked = []
    for app_dir in sorted(APPS_DIR.iterdir()):
        main = app_dir / "src" / "main.rs"
        if not main.is_file() or app_dir.name not in spawns:
            continue
        code = code_only(main.read_text(encoding="utf-8"))
        draws_band_clock = (
            "StatusBand::new(" in code and "rtc::Reading::now()" in code
        )
        if not draws_band_clock:
            continue
        checked.append(app_dir.name)
        assert "CapabilityType::RealtimeClockRead" in spawns[app_dir.name], (
            f"{app_dir.name} paints the band's clock lane from the RTC but the kernel "
            f"grants it no RealtimeClockRead, so the lane is always refused and blank"
        )

    # The scan has to actually be looking at the apps that adopted the band.
    assert len(checked) >= 10, (
        f"only {len(checked)} apps were checked; the band is adopted by at least ten, "
        "so the adoption markers this scan reads have moved"
    )


def test_the_clock_answer_is_long_enough_to_reach_the_instant():
    """The kernel's RTC answer and the reader's buffer are one number.

    WHY this crosses the boundary: the kernel truncates its JSON to the length
    the caller offered and refuses anything longer, so the two halves have to
    agree byte for byte. At 64 the answer stopped inside `"second"` and
    `unix_epoch` -- the field at the end -- could never be read by anyone, which
    is what left the TLS client unable to tell an expired certificate from a
    live one. A silent truncation looks exactly like a clock with no instant.
    """
    syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    arm = syscall[syscall.index("38 => {"):]
    arm = arm[: arm.index("40 => {")]
    guard = re.search(r"if len > (\d+) \{\n\s*return SyscallResult::InvalidArguments", arm)
    assert guard, "the RtcGetTime arm must bound the length a caller may request"
    kernel_cap = int(guard.group(1))
    assert f"let mut kbuf = [0u8; {kernel_cap}];" in arm, (
        f"the kernel's own staging buffer must be the {kernel_cap} bytes it accepts"
    )

    sdk = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
    declared = re.search(r"pub const RTC_PAYLOAD_BYTES: usize = (\d+);", sdk)
    assert declared, "the reader must name the RTC answer size it depends on"
    assert int(declared.group(1)) == kernel_cap, (
        f"the SDK reads {declared.group(1)} bytes but the kernel refuses more than "
        f"{kernel_cap}; one of them silently loses the tail of the clock answer"
    )
    assert "let mut buf = [0u8; super::RTC_PAYLOAD_BYTES];" in sdk, (
        "the shared clock reader must size its buffer from that constant"
    )

    # The instant is only worth reading if a client that must judge an absolute
    # deadline actually reaches for it.
    browser = (ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8")
    assert "reading.unix_utc()" in browser, (
        "the browser must take the UTC instant out of the clock reading it is "
        "the only way a certificate's expiry can be judged on the device"
    )


if __name__ == "__main__":
    test_sdk_exports_status_band()
    test_status_band_behavior_and_pixels_pass()
    test_all_target_apps_adopt_status_band()
    test_band_renders_no_state_without_backing_query()
    test_every_app_treats_unavailable_as_unavailable()
    test_cellular_mark_requires_measured_level_not_modem_presence()
    test_file_manager_adopts_band_with_only_obtainable_instruments()
    test_an_app_that_draws_the_band_clock_lane_holds_the_clock_read()
    test_the_clock_answer_is_long_enough_to_reach_the_instant()
    print("Status band truth contract: PASS")
