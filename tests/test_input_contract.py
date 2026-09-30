#!/usr/bin/env python3
"""Static contracts for normalized userspace input."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def main():
    inp=(ROOT/"kernel/src/drivers/input.rs").read_text()
    drv=(ROOT/"kernel/src/drivers/mod.rs").read_text()
    sys=(ROOT/"kernel/src/syscall/mod.rs").read_text()
    sdk=(ROOT/"sdk/rust/interos-std/src/lib.rs").read_text()
    svc=(ROOT/"userspace/system-service/src/lib.rs").read_text()
    binp=(ROOT/"userspace/system-service/src/bin/inputd.rs").read_text()
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

if __name__ == "__main__":
    main()


def test_first_party_apps_accept_canonical_system_gestures():
    from pathlib import Path
    root = Path(__file__).resolve().parents[1]
    for name in ("settings","terminal","browser","camera","calculator","calendar","dialer","file_manager","gallery","messages","music","vault"):
        source = (root / "apps" / name / "src/main.rs").read_text()
        assert "input.kind == 6" in source, name
        assert "input.kind == 7" in source, name
        assert "input.kind == 8" in source, name


def test_gesture_thresholds_match_kernel_tick_rate():
    from pathlib import Path
    root = Path(__file__).resolve().parents[1]
    interrupts = (root / "kernel/src/interrupts.rs").read_text()
    router = (root / "kernel/src/input/router.rs").read_text()
    assert "SYSTEM_TICK_HZ" in interrupts
    assert "LONG_PRESS_TICKS: u64 = 500" in router
    assert "DOUBLE_TAP_WINDOW_TICKS: u64 = 300" in router
    assert "FLING_WINDOW_TICKS: u64 = 250" in router
