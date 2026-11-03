# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

$logFile = "target\boot_run.log"
if (Test-Path $logFile) { Remove-Item $logFile -Force }

$rootDir = (Resolve-Path "$PSScriptRoot\..").Path
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemuBin = Resolve-QemuPath "qemu-system-x86_64.exe"
$p = Start-Process -FilePath $qemuBin -ArgumentList "-machine q35 -cpu qemu64 -m 1024M -drive format=raw,file=target\inter-os-bios.img -serial file:$logFile -display none -no-reboot" -PassThru
Start-Sleep -Seconds 3
Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue

if (Test-Path $logFile) {
    Get-Content $logFile
} else {
    Write-Error "Log file not found!"
}
