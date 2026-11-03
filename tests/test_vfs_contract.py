# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path
ROOT=Path(__file__).parents[1]
v=(ROOT/"kernel/src/fs/vfs.rs").read_text(encoding="utf-8"); s=(ROOT/"kernel/src/syscall/mod.rs").read_text(encoding="utf-8")
assert "MAX_FILE_SIZE: usize = 8192" in v
assert "MAX_PATH_LEN: usize = 256" in v
assert "PathTooLong" in v and "FileTooLarge" in v
assert "buf.len() > MAX_FILE_SIZE" in v
assert "checked_add(buf.len())" in v
assert "end > MAX_FILE_SIZE" in v
assert "FsError::FileTooLarge | FsError::PathTooLong" in s
assert "pub fn mkdir_for_task" in v
assert "FsMkdir = 161" in s
assert "pub fn sys_fs_mkdir" in s
print("VFS boundary contract: PASS")
