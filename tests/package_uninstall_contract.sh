#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
grep -q 'PackageUninstall = 120' kernel/src/syscall/mod.rs
grep -q 'sys_package_uninstall' kernel/src/syscall/mod.rs
grep -q 'pub fn uninstall(&mut self' kernel/src/package/registry.rs
grep -q 'has_active_dependents' kernel/src/package/registry.rs
grep -q 'remove_app_storage' kernel/src/fs/vfs.rs
grep -q 'PACKAGE_BLOB_STORE.lock().remove' kernel/src/syscall/mod.rs
grep -q 'package_uninstall' sdk/rust/interos-std/src/lib.rs
echo "package uninstall contract: PASS"
