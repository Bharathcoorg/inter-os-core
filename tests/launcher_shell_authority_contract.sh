#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
grep -q 'AppRuntimeList = 123' kernel/src/syscall/mod.rs
grep -q 'AppFocus = 124' kernel/src/syscall/mod.rs
grep -q 'sys_app_runtime_list' kernel/src/syscall/mod.rs
grep -q 'sys_app_focus' kernel/src/syscall/mod.rs
grep -q 'tasks_iter' kernel/src/task/scheduler.rs
grep -q 'app_runtime_list' sdk/rust/interos-std/src/lib.rs
grep -q 'app_focus' sdk/rust/interos-std/src/lib.rs
grep -q 'refresh_runtime_recents' apps/launcher/src/main.rs
grep -q 'std::app_focus' apps/launcher/src/main.rs
grep -q 'std::app_stop' apps/launcher/src/main.rs
echo "launcher shell authority contract: PASS"
