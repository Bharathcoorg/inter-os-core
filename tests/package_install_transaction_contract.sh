#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
grep -q 'previous_registry_entry' kernel/src/syscall/mod.rs
grep -q 'rollback_install' kernel/src/syscall/mod.rs
grep -q 'pub fn rollback_install' kernel/src/package/registry.rs
grep -q 'pub fn restore_entry' kernel/src/package/registry.rs
grep -q 'PRG1' kernel/src/package/registry.rs
grep -q 'pos != len' kernel/src/package/registry.rs
echo "package install transaction contract: PASS"
