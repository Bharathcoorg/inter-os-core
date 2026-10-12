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

    must(SCHED, "crate::event::cleanup_task(task_id);", "current-task event cleanup")
    must(SCHED, "crate::event::cleanup_task(id);", "managed-task event cleanup")

    assert SCHED.index("crate::ipc::cleanup_task(task_id);") < SCHED.index("crate::event::cleanup_task(task_id);"), "IPC cleanup remains before event cleanup"
    assert SCHED.index("crate::fs::cleanup_task(task_id);") < SCHED.index("crate::event::cleanup_task(task_id);"), "VFS cleanup remains before event cleanup"
    print("event lifecycle contract: PASS")


if __name__ == "__main__":
    main()
