#!/usr/bin/env bash
set -euo pipefail

grep -q 'AppStorageOpen = 115' kernel/src/syscall/mod.rs
grep -q 'sys_app_storage_open' kernel/src/syscall/mod.rs
grep -q 'app_storage_open' sdk/rust/interos-std/src/lib.rs
grep -q 'enforce_app_path' kernel/src/fs/vfs.rs
grep -q '/user/{}/apps/{}' kernel/src/fs/vfs.rs
grep -q 'user_for_task' kernel/src/fs/vfs.rs
grep -q 'bind_user_for_task' kernel/src/task/process.rs
grep -q 'spawn_user_process_with_parent(process_name' kernel/src/task/user.rs
grep -q 'starts_with("/user/")' kernel/src/fs/vfs.rs

echo "app storage sandbox contract: PASS"
