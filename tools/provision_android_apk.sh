#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>
#
# Provisions the external Android acceptance APK used by the Android
# contract suites. The APK is an upstream release artifact (Fossify File
# Manager 1.6.1, GPLv3+) and is deliberately not committed: it is ignored
# under emulator/data/apps/.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APK_PATH="$ROOT/emulator/data/apps/fossify_file_manager.apk"
APK_URL="https://github.com/FossifyOrg/File-Manager/releases/download/1.6.1/file-manager-13-foss-release.apk"
APK_SHA256="9e97d2faf55fb2702386c3dc51e38a6b073781016b125701d3ab668419a70008"

if [[ -f "$APK_PATH" ]]; then
    actual="$(sha256sum "$APK_PATH" | awk '{print $1}')"
    if [[ "$actual" == "$APK_SHA256" ]]; then
        echo "APK already provisioned and verified: $APK_PATH"
        exit 0
    fi
    echo "Existing APK digest mismatch ($actual); refusing to use it." >&2
    exit 1
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl --fail --location --silent --show-error --output "$tmp" "$APK_URL"

actual="$(sha256sum "$tmp" | awk '{print $1}')"
if [[ "$actual" != "$APK_SHA256" ]]; then
    echo "REFUSING to install: SHA-256 mismatch." >&2
    echo "  expected: $APK_SHA256" >&2
    echo "  actual:   $actual" >&2
    exit 1
fi

mkdir -p "$(dirname "$APK_PATH")"
mv "$tmp" "$APK_PATH"
trap - EXIT
echo "Provisioned $APK_PATH (SHA-256 verified)"
