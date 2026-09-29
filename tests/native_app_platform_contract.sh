#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
grep -q 'pub struct PackageDescriptor' kernel/src/package/mod.rs
grep -q 'display_name: &' kernel/src/package/mod.rs
grep -q 'icon: &' kernel/src/package/mod.rs
grep -q 'launch_allowed(app_id' kernel/src/syscall/mod.rs
grep -q 'pub struct AppSession' sdk/rust/interos-std/src/app.rs
grep -q 'pub mod app;' sdk/rust/interos-std/src/lib.rs
echo "native application platform contract: PASS"
