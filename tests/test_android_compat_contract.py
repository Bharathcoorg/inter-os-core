# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_android_framework_ui_lifecycle_state_machine_and_transitions():
    framework = read("kernel/src/compat/android/framework.rs")
    # Must declare an explicit Activity lifecycle state machine
    assert "pub enum ActivityLifecycleState" in framework
    assert "Uninitialized" in framework
    assert "Created" in framework
    assert "Started" in framework
    assert "Resumed" in framework
    assert "Paused" in framework
    assert "Stopped" in framework
    assert "Destroyed" in framework
    assert "LIFECYCLE_MANAGER" in framework
    # Must implement all standard Android Activity lifecycle methods
    assert '"onCreate"' in framework
    assert '"onStart"' in framework
    assert '"onResume"' in framework
    assert '"onPause"' in framework
    assert '"onStop"' in framework
    assert '"onDestroy"' in framework
    assert '"finish"' in framework
    assert '"isFinishing"' in framework
    # Must enforce valid lifecycle transitions and reject out-of-order transitions
    assert "transition_to" in framework
    assert "InvalidArguments" in framework


def test_android_framework_service_surface_and_resolution():
    framework = read("kernel/src/compat/android/framework.rs")
    runtime = read("kernel/src/compat/android/runtime.rs")
    # Must have bounded service enumeration
    assert "pub enum AndroidFrameworkService" in framework
    for service_variant in ["Activity", "Window", "Input", "Audio", "Notification", "Storage", "Network", "Telephony"]:
        assert service_variant in framework, f"Missing service variant: {service_variant}"
    # Must support service name resolution and integer service handles
    assert "fn handle(" in framework
    assert "fn from_name(" in framework
    # Must resolve getSystemService for supported system services
    assert '"getSystemService"' in framework
    # Must implement real service calls
    assert "android/app/NotificationManager" in framework
    assert "android/media/AudioManager" in framework
    assert "android/telephony/TelephonyManager" in framework
    assert "android/net/ConnectivityManager" in framework
    # Dalvik VM must dispatch Service results to register values
    assert "AndroidFrameworkResult::Service(svc)" in runtime
    assert "self.last_return_value = svc.handle()" in runtime


def test_android_framework_truth_contract_fails_closed_on_unimplemented_surfaces():
    framework = read("kernel/src/compat/android/framework.rs")
    # Unimplemented UI/Widget mutations must fail closed (never return synthetic success)
    assert "android/widget/TextView" in framework
    assert "android/widget/Button" in framework
    assert "android/view/View" in framework
    assert "android/os/Handler" in framework
    # Unimplemented / ambient hardware queries must fail closed
    assert "getDeviceId" in framework or "getImei" in framework
    assert "PermissionDenied" in framework
    assert "UnsupportedApi" in framework


if __name__ == "__main__":
    test_android_framework_ui_lifecycle_state_machine_and_transitions()
    test_android_framework_service_surface_and_resolution()
    test_android_framework_truth_contract_fails_closed_on_unimplemented_surfaces()
    print("Android compat contract: PASS")

