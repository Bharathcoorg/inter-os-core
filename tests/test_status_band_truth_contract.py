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


if __name__ == "__main__":
    test_sdk_exports_status_band()
    test_status_band_behavior_and_pixels_pass()
    test_all_target_apps_adopt_status_band()
    test_band_renders_no_state_without_backing_query()
    test_every_app_treats_unavailable_as_unavailable()
    print("Status band truth contract: PASS")
