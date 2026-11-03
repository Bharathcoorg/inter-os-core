#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

set -euo pipefail

grep -q 'first_party_package_by_process_name' kernel/src/package/mod.rs
grep -q 'active_entries' kernel/src/package/registry.rs
grep -q 'sys_package_registry' kernel/src/syscall/mod.rs
grep -q 'PACKAGE_REGISTRY.lock().seed()' kernel/src/syscall/mod.rs
grep -q 'package_registry(0' apps/launcher/src/main.rs
grep -q 'app_ids' apps/launcher/src/main.rs
grep -q 'resolve_app_in_drawer(&state' apps/launcher/src/main.rs

echo "package registry authority contract: PASS"
