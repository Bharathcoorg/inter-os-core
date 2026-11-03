# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EVENT = (ROOT / "kernel/src/event/mod.rs").read_text(encoding="utf-8")
SCHED = (ROOT / "kernel/src/task/scheduler.rs").read_text(encoding="utf-8")


def must(text: str, needle: str, label: str) -> None:
    assert needle in text, f"missing {label}: {needle}"


def main() -> None:
    must(EVENT, "pub fn cleanup_task(&mut self, task_id: TaskId) -> bool", "event subscription cleanup")
    must(EVENT, "self.subscriptions.retain(|sub| sub.task_id != task_id)", "dead subscription removal")
    must(EVENT, "pub fn cleanup_task(task_id: TaskId)", "event cleanup wrapper")
    must(EVENT, "EVENT_REACTOR.lock().cleanup_task(task_id);", "global event cleanup")

    must(SCHED, "crate::event::cleanup_task(entry.id);", "task event cleanup")
    assert SCHED.count("defer_cleanup(") >= 2, "both termination paths queue deferred cleanup"

    assert SCHED.index("crate::ipc::cleanup_task(entry.id);") < SCHED.index("crate::event::cleanup_task(entry.id);"), "IPC cleanup remains before event cleanup"
    assert SCHED.index("crate::fs::cleanup_task(entry.id);") < SCHED.index("crate::event::cleanup_task(entry.id);"), "VFS cleanup remains before event cleanup"
    print("event lifecycle contract: PASS")


if __name__ == "__main__":
    main()
