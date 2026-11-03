# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

$rootDir = (Resolve-Path "$PSScriptRoot\..").Path
$img = "$rootDir\target\inter-os-bios.img"
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemu = Resolve-QemuPath "qemu-system-x86_64.exe"

Write-Host "Launching QEMU SDL GUI with live interactive window..." -ForegroundColor Cyan
& $qemu -machine q35 -cpu qemu64 -m 1024M -drive format=raw,file=$img -display sdl -serial stdio -monitor tcp:127.0.0.1:44445,server,nowait -name "InterOS Core v0.1"
