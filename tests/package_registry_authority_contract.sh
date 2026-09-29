#!/usr/bin/env bash
set -euo pipefail

grep -q 'first_party_package_by_process_name' kernel/src/package/mod.rs
grep -q 'active_entries' kernel/src/package/registry.rs
grep -q 'sys_package_registry' kernel/src/syscall/mod.rs
grep -q 'PACKAGE_REGISTRY.lock().seed()' kernel/src/syscall/mod.rs
grep -q 'package_registry(0' apps/launcher/src/main.rs
grep -q 'app_ids' apps/launcher/src/main.rs
grep -q 'resolve_app_in_drawer(&state' apps/launcher/src/main.rs

echo "package registry authority contract: PASS"
