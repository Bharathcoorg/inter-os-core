#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Static contracts for normalized userspace input."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def test_input_pipeline_contract():
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


def test_inputd_service_drain_contract():
    """Verify inputd drains service_queue to prevent saturation while apps poll their own queues."""
    svc = (ROOT / "userspace/system-service/src/lib.rs").read_text(encoding="utf-8")
    sys = (ROOT / "kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
    inp = (ROOT / "kernel/src/drivers/input.rs").read_text(encoding="utf-8")

    # inputd loop drains service queue
    must(svc, "pub fn run_input() -> !", "inputd service function")
    must(svc, "while std::input_event_poll(&mut event).is_ok() {", "inputd drain loop")
    must(svc, "// Drain normalized service input events to prevent queue saturation", "drain comment")

    # Kernel differentiates inputd service from ordinary tasks
    must(sys, "let is_inputd = crate::services::task_id(crate::services::ServiceId::Input) == Some(task.id);", "inputd task check")
    must(sys, "INPUT_PIPELINE.lock().pop_service().ok_or(SyscallResult::NotReady)?", "service queue pop")
    must(sys, "INPUT_PIPELINE.lock().pop_for_task(task_id).ok_or(SyscallResult::NotReady)?", "per-task queue pop")

    # Pop for task enforces task isolation
    must(inp, "pub fn pop_for_task(&mut self, task: TaskId) -> Option<InputEvent>", "pop_for_task signature")
    must(inp, "let index = self.queue.iter().position(|(owner, _)| *owner == task)?;", "task owner filter")


def test_focused_task_dispatch_and_no_thundering_herd():
    """Verify pointer dispatch wakes ONLY the owning task, avoiding idle-wake thundering herds."""
    inp = (ROOT / "kernel/src/drivers/input.rs").read_text(encoding="utf-8")
    router = (ROOT / "kernel/src/input/router.rs").read_text(encoding="utf-8")
    mod_input = (ROOT / "kernel/src/input/mod.rs").read_text(encoding="utf-8")

    # Only post_event to owner, not broadcast_event
    must(inp, "crate::event::post_event(TaskId(owner), crate::event::EVENT_TOUCH_INPUT);", "targeted task wake")
    must(inp, "pipe.route_to_task(TaskId(owner), event);", "targeted queue routing")
    must(inp, "router.send_direct(TaskId(0), TaskId(owner), 0x494E_5054, &msg);", "targeted IPC dispatch")

    # router suppresses wobble within TAP_SLOP on Move
    must(router, "const TAP_SLOP: i32 = 24;", "tap slop definition")
    must(router, "if total_dist > TAP_SLOP && (step_dx != 0 || step_dy != 0)", "scroll only after slop")

    # pointer contact dispatches to focused window only when a gesture is recognized
    must(mod_input, "if let Some(gesture) = gesture {", "gesture guard on dispatch")
    must(mod_input, "crate::drivers::input::dispatch_to_focused_window(event);", "dispatch on gesture")


def test_queue_overflow_and_backpressure_bounds():
    """Verify queue bounds and non-allocating ring buffer invariants."""
    inp = (ROOT / "kernel/src/drivers/input.rs").read_text(encoding="utf-8")
    queue = (ROOT / "kernel/src/input/queue.rs").read_text(encoding="utf-8")
    ps2 = (ROOT / "kernel/src/drivers/ps2_mouse.rs").read_text(encoding="utf-8")

    must(inp, "const MAX_PER_TASK: usize = 16;", "max per-task queue limit")
    must(inp, "const MAX_TOTAL: usize = 128;", "max total input pipeline limit")
    must(queue, "pub const INPUT_QUEUE_CAPACITY: usize = 64;", "hardware ring buffer capacity")
    must(queue, "self.dropped = self.dropped.saturating_add(1);", "overflow drop counter")

    # PS/2 driver drops Move when full to coalesce pointer motion
    must(ps2, "const EVENT_CAPACITY: usize = 64;", "PS/2 event capacity")
    must(ps2, "if phase == 1 { return; }", "PS/2 move coalescing on saturation")


def test_pointer_touch_and_cancel_invariants():
    """Verify Down, Move, Up, and Cancel phases map deterministically to pointer and gesture events."""
    touch = (ROOT / "kernel/src/input/touch.rs").read_text(encoding="utf-8")
    router = (ROOT / "kernel/src/input/router.rs").read_text(encoding="utf-8")
    serial = (ROOT / "kernel/src/serial.rs").read_text(encoding="utf-8")

    # Touch phases enum
    must(touch, "pub enum TouchPhase {", "TouchPhase definition")
    must(touch, "Down,", "Down phase")
    must(touch, "Move,", "Move phase")
    must(touch, "Up,", "Up phase")
    must(touch, "Cancel,", "Cancel phase")

    # Serial protocol parser supports Cancel
    must(serial, 'Some("CANCEL") => crate::input::TouchPhase::Cancel,', "serial Cancel support")

    # Router handles Cancel
    must(router, "PointerPhase::Cancel => {", "router Cancel branch")
    must(router, "kind: GestureKind::Cancel,", "Cancel gesture emission")


def main():
    test_input_pipeline_contract()
    test_inputd_service_drain_contract()
    test_focused_task_dispatch_and_no_thundering_herd()
    test_queue_overflow_and_backpressure_bounds()
    test_pointer_touch_and_cancel_invariants()
    print("input event pipeline contract: PASS")

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
