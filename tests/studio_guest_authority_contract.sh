#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
grep -q 'STUDIO_PKG_LIST' kernel/src/serial.rs
grep -q 'STUDIO_FS_LIST' kernel/src/serial.rs
grep -q 'STUDIO_FS_READ' kernel/src/serial.rs
grep -q 'STUDIO_FS_BEGIN' kernel/src/serial.rs
grep -q 'STUDIO_FS_CHUNK' kernel/src/serial.rs
grep -q 'STUDIO_FS_COMMIT' kernel/src/serial.rs
grep -q 'DELETE_FILE' kernel/src/serial.rs
grep -q 'MKDIR' kernel/src/serial.rs
grep -q 'guest_package_catalog' emulator/server.py
grep -q 'guest_fs_list' emulator/server.py
grep -q 'guest_fs_read' emulator/server.py
grep -q 'guest_fs_write' emulator/server.py
grep -q "'source': 'GUEST_VFS'" emulator/server.py
grep -q "'source': 'GUEST_REGISTRY'" emulator/server.py
! grep -q 'os.remove(app_file)' emulator/server.py
! grep -q 'open(real_path' emulator/server.py
echo "Device Studio guest-authority contract: PASS"
