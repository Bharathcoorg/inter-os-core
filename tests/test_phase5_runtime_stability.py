#!/usr/bin/env python3
"""Phase 5 runtime-stability contracts.

These checks lock the cold-boot/runtime lifecycle invariants without claiming
fresh QEMU or physical-device execution.
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def main():
    queue = (ROOT / "kernel/src/input/queue.rs").read_text()
    event = (ROOT / "kernel/src/event/mod.rs").read_text()
    process = (ROOT / "kernel/src/task/process.rs").read_text()
    scheduler = (ROOT / "kernel/src/task/scheduler.rs").read_text()
    kms = (ROOT / "kernel/src/drivers/display_kms.rs").read_text()
    display = (ROOT / "kernel/src/drivers/display.rs").read_text()
    lifecycle = (ROOT / "tests/test_interactive_lifecycle.py").read_text()

    must(queue, "dropped: u64", "input overflow accounting")
    must(queue, "self.dropped = self.dropped.saturating_add(1)", "input overflow increment")
    must(queue, "pub fn dropped_count(&self) -> u64", "input drop telemetry")

    must(event, "for sub in self.subscriptions.iter_mut()", "unbounded event waiter scan")
    assert "to_unblock = [None; 16]" not in event, "event reactor still has a 16-waiter cap"
    must(event, "sub.is_waiting = false", "event waiter release")
    must(event, "EVENT_TIMER_TICK", "timer wakeup contract")

    must(process, "actual_threads = threads.iter().flatten().filter(|t| t.process == record.id).count()", "process thread accounting")
    must(process, "if actual_threads != record.thread_count as usize", "thread-count invariant")
    must(process, "if !table.iter().flatten().any(|record| record.id == thread.process)", "thread binding invariant")

    must(scheduler, "crate::ipc::cleanup_task(task_id);", "exit IPC cleanup")
    must(scheduler, "crate::fs::cleanup_task(task_id);", "exit filesystem cleanup")
    must(scheduler, "crate::event::cleanup_task(task_id);", "exit event cleanup")
    must(scheduler, "crate::net::cleanup_task(task_id);", "exit network cleanup")
    must(scheduler, "destroy_surfaces_owned_by(task_id.0)", "exit surface cleanup")
    must(scheduler, "task.parent_id = TaskId(0);", "orphan reparenting")

    must(kms, "MAX_CLIENT_SURFACES: usize = 32", "surface count bound")
    must(kms, "MAX_SURFACE_PIXELS: usize = 1280 * 720", "surface pixel bound")
    must(kms, "checked_add(1)", "surface id overflow guard")
    must(display, 'return Err("display surface capacity exhausted")', "surface allocation failure propagation")

    must(lifecycle, "Cold boot", "cold-boot lifecycle coverage")
    must(lifecycle, "launcher", "launcher lifecycle coverage")
    must(lifecycle, "reaped", "child reaping coverage")
    must(lifecycle, "overview", "launcher recovery/overview coverage")

    print("phase 5 runtime stability contract: PASS")

if __name__ == "__main__":
    main()
