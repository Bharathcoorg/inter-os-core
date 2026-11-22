#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

# POSIX convenience wrapper. The canonical, cross-platform entry point is:
#   python tools/interos.py provision android-apk
# The pinned URL, SHA-256 and fail-closed verification live in
# tools/provisioning.py so there is a single source of provenance truth.
set -euo pipefail
exec python3 "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/interos.py" provision android-apk "$@"
