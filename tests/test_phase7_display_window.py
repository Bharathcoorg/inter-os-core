#!/usr/bin/env python3
"""Phase 7 display/window-system contracts.

Static contracts validate the ownership and input-routing boundary without
claiming fresh QEMU or physical-device execution.
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def main():
    kms = (ROOT / "kernel/src/drivers/display_kms.rs").read_text()
    inputd = (ROOT / "kernel/src/drivers/input.rs").read_text()
    syscall = (ROOT / "kernel/src/syscall/mod.rs").read_text()
    sdk = (ROOT / "sdk/rust/interos-std/src/lib.rs").read_text()

    must(kms, "owner_task: u64", "window ownership")
    must(kms, "input_enabled: bool", "window input participation")
    must(kms, "set_input_enabled_owned", "owned input participation mutation")
    must(kms, "focus_at", "window hit testing")
    must(kms, "set_surface_geometry_owned", "owned geometry mutation")
    must(kms, "destroy_surfaces_owned_by", "window lifecycle cleanup")
    must(kms, "surface_snapshot", "window metadata snapshot")

    must(inputd, "VecDeque<(TaskId, InputEvent)>", "per-task input queue")
    must(inputd, "route_to_task", "focused-window input routing")
    must(inputd, "pop_for_task", "task-scoped input consumption")
    must(inputd, "dispatch_to_focused_window", "focus-based dispatch")

    must(syscall, "WindowSetInput = 104", "window input syscall")
    must(syscall, "sys_window_set_input", "owned input syscall handler")
    must(syscall, "set_input_enabled_owned", "kernel ownership enforcement")
    must(syscall, "pop_for_task(task_id)", "caller-scoped input polling")
    must(syscall, "WindowList", "window inspection ABI")
    must(syscall, "WindowSetGeometry", "window geometry ABI")
    must(syscall, "WindowFocus", "window focus ABI")

    must(sdk, "pub fn window_set_input", "SDK window input API")
    must(sdk, "SyscallNumber::WindowSetInput", "SDK syscall binding")

    print("phase 7 display/window system contract: PASS")

if __name__ == "__main__":
    main()
