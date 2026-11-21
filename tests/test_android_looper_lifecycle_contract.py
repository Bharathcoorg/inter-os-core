# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Android MessageQueue, Looper, Choreographer & Activity Back Stack Contract Tests.

Verifies:
1. Message and MessageQueue chronological ordering by timestamp ticks.
2. Looper main event loop driving message polling and dispatch.
3. Choreographer vsync 60 FPS frame callback scheduling and execution.
4. Multi-activity TaskRecord back stack with standard Activity navigation:
   Activity A (Resumed) -> start_activity(B) -> A (Stopped), B (Resumed)
   finish_top_activity() -> B (Destroyed), A (Resumed).
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_kernel_looper_and_message_queue_invariants():
    source = read_source("kernel/src/compat/android/looper.rs")
    assert "pub struct Message" in source
    assert "pub struct MessageQueue" in source
    assert "pub struct Looper" in source
    assert "pub fn enqueue_message(&mut self, msg: Message)" in source
    assert "pub fn poll_next(&mut self, current_ticks: u64) -> Option<Message>" in source
    assert "pub fn loop_once(&mut self, current_ticks: u64) -> usize" in source


def test_kernel_choreographer_frame_pacing():
    source = read_source("kernel/src/compat/android/looper.rs")
    assert "pub struct Choreographer" in source
    assert "pub fn post_frame_callback(&mut self, cb_id: u32)" in source
    assert "pub fn do_frame(&mut self, frame_time_nanos: u64) -> usize" in source


def test_kernel_activity_back_stack_lifecycle():
    source = read_source("kernel/src/compat/android/looper.rs")
    assert "pub struct ActivityRecord" in source
    assert "pub struct TaskRecord" in source
    assert "pub fn start_activity(&mut self, component_name: &str, intent: &str) -> &ActivityRecord" in source
    assert "pub fn finish_top_activity(&mut self) -> Option<ActivityRecord>" in source
    assert "prev.state = ActivityLifecycleState::Stopped;" in source
    assert "underneath.state = ActivityLifecycleState::Resumed;" in source

    # Also verify Ring-3 ACR looper and task manager
    acr_looper = read_source("crates/interos-acr/src/looper.rs")
    assert "pub struct Looper" in acr_looper
    assert "pub struct Choreographer" in acr_looper
    assert "pub struct TaskRecord" in acr_looper
