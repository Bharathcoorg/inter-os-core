#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
grep -q 'refresh_package_catalog' apps/launcher/src/main.rs
grep -q 'resolve_app_at_touch(&state' apps/launcher/src/main.rs
grep -q 'recent_app_lens' apps/launcher/src/main.rs
grep -q 'display_name' kernel/src/syscall/mod.rs
for app in calculator vault browser messages calendar music; do
  if grep -q 'Some("org.interos.'"$app"'"' apps/launcher/src/main.rs; then
    echo "fixed home-grid identity remains: $app" >&2
    exit 1
  fi
done
echo "launcher registry authority contract: PASS"
