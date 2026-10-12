from pathlib import Path
ROOT=Path(__file__).parents[1]
v=(ROOT/"kernel/src/fs/vfs.rs").read_text(encoding="utf-8"); m=(ROOT/"kernel/src/fs/mod.rs").read_text(encoding="utf-8"); s=(ROOT/"kernel/src/task/scheduler.rs").read_text(encoding="utf-8")
assert "pub fn cleanup_task(&mut self, task_id: TaskId) -> usize" in v
assert "self.task_file_tables.remove(&task_id)" in v
assert "pub fn cleanup_task(task_id: crate::task::TaskId) -> usize" in m
assert "crate::fs::cleanup_task(task_id);" in s
print("VFS task-lifecycle contract: PASS")
