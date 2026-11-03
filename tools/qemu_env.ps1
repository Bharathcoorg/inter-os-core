# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

function Resolve-QemuPath {
    param([string]$BinaryName = "qemu-system-x86_64.exe")
    if ($env:QEMU_PATH -and (Test-Path (Join-Path $env:QEMU_PATH $BinaryName))) { return Join-Path $env:QEMU_PATH $BinaryName }
    if ($env:QEMU_DIR -and (Test-Path (Join-Path $env:QEMU_DIR $BinaryName))) { return Join-Path $env:QEMU_DIR $BinaryName }
    $found = Get-Command $BinaryName -ErrorAction SilentlyContinue
    if ($found) { return $found.Source }
    $candidates = @(
        (Join-Path $HOME "qemu\$BinaryName"),
        "C:\Program Files\qemu\$BinaryName",
        "C:\qemu\$BinaryName"
    )
    foreach ($cand in $candidates) {
        if (Test-Path $cand) { return $cand }
    }
    return $BinaryName
}
