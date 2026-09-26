from pathlib import Path
ROOT=Path(__file__).parents[1]
v=(ROOT/"kernel/src/fs/vfs.rs").read_text(); s=(ROOT/"kernel/src/syscall/mod.rs").read_text()
assert "MAX_FILE_SIZE: usize = 8192" in v
assert "MAX_PATH_LEN: usize = 256" in v
assert "PathTooLong" in v and "FileTooLarge" in v
assert "buf.len() > MAX_FILE_SIZE" in v
assert "checked_add(buf.len())" in v
assert "end > MAX_FILE_SIZE" in v
assert "FsError::FileTooLarge | FsError::PathTooLong" in s
print("VFS boundary contract: PASS")
