#!/usr/bin/env python3
"""Static contracts for normalized userspace input."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def main():
    inp=(ROOT/"kernel/src/drivers/input.rs").read_text(encoding="utf-8")
    drv=(ROOT/"kernel/src/drivers/mod.rs").read_text(encoding="utf-8")
    sys=(ROOT/"kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    sdk=(ROOT/"sdk/rust/interos-std/src/lib.rs").read_text(encoding="utf-8")
    svc=(ROOT/"userspace/system-service/src/lib.rs").read_text(encoding="utf-8")
    binp=(ROOT/"userspace/system-service/src/bin/inputd.rs").read_text(encoding="utf-8")
    for n in ("pub enum InputEventKind","pub struct InputEvent","pub struct InputPipeline",
              "pub fn pump_touch_events","pub fn dispatch_to_focused_window","INPUT_PIPELINE"):
        must(inp,n,n)
    must(drv,"pub mod input;","input driver module")
    must(sys,"InputEventPoll = 92","input syscall number")
    must(sys,"pub fn sys_input_event_poll","input syscall implementation")
    must(sys,"CapabilityType::EventPoll, Rights::READ","input capability gate")
    must(sys,"crate::drivers::input::pump_touch_events()","touch-to-event bridge")
    must(sys,"out_buf[0] = event.kind as u8","bounded event wire encoding")
    must(inp,"send_direct(TaskId(0), TaskId(owner), 0x494E_5054","focused-window input IPC")
    must(sdk,"pub struct InputEvent","userspace event type")
    must(sdk,"SyscallNumber::InputEventPoll","userspace input ABI")
    must(svc,"pub fn run_input() -> !","dedicated input service")
    must(svc,"std::input_event_poll(&mut event)","input service uses normalized ABI")
    must(binp,"interos_system_service::run_input()","inputd entry point")
    print("input event pipeline contract: PASS")

    # WHY these are called here as well as collected by pytest: this file is run
    # both as a standalone contract and as a test module. They were renamed to
    # `test_*` so pytest would collect them, which left main() calling names that no
    # longer existed and every run died with a NameError before asserting anything.
    test_first_party_apps_accept_canonical_system_gestures()
    test_gesture_thresholds_match_kernel_tick_rate()
    print("first-party gesture and timing contracts: PASS")



def test_first_party_apps_accept_canonical_system_gestures():
    from pathlib import Path
    root = Path(__file__).resolve().parents[1]
    for name in ("settings","browser","camera","calculator","calendar","dialer","file_manager","gallery","messages","music","vault"):
        source = (root / "apps" / name / "src/main.rs").read_text(encoding="utf-8")
        assert "input.kind == 6" in source, name
        assert "input.kind == 7" in source, name
        assert "input.kind == 8" in source, name


def test_gesture_thresholds_match_kernel_tick_rate():
    from pathlib import Path
    root = Path(__file__).resolve().parents[1]
    interrupts = (root / "kernel/src/interrupts.rs").read_text(encoding="utf-8")
    router = (root / "kernel/src/input/router.rs").read_text(encoding="utf-8")
    assert "SYSTEM_TICK_HZ" in interrupts
    assert "LONG_PRESS_TICKS: u64 = 500" in router
    assert "DOUBLE_TAP_WINDOW_TICKS: u64 = 300" in router
    assert "FLING_WINDOW_TICKS: u64 = 250" in router

if __name__ == "__main__":
    main()
