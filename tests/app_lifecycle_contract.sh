#!/usr/bin/env bash
set -euo pipefail

grep -q 'ProcessState { Running = 0, Background = 1, Suspended = 2, Killed = 3, Restarting = 4' kernel/src/task/process.rs
grep -q 'max_background_cpu_ticks' kernel/src/task/process.rs
grep -q 'set_background' kernel/src/task/process.rs
grep -q 'set_foreground' kernel/src/task/process.rs
grep -q 'enforce_background_budget' kernel/src/task/process.rs
grep -q 'AppBackground = 116' kernel/src/syscall/mod.rs
grep -q 'AppForeground = 117' kernel/src/syscall/mod.rs
grep -q 'AppSuspend = 118' kernel/src/syscall/mod.rs
grep -q 'AppWake = 119' kernel/src/syscall/mod.rs
grep -q 'app_background' sdk/rust/interos-std/src/lib.rs
grep -q 'app_suspend' sdk/rust/interos-std/src/lib.rs
grep -q 'wake_package' kernel/src/notification/mod.rs

echo "application lifecycle contract: PASS"
