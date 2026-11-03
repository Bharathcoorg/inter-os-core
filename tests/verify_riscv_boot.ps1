# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

$rootDir = Split-Path -Parent $PSScriptRoot
Push-Location (Join-Path $rootDir "kernel-riscv64")
cargo build --target riscv64gc-unknown-none-elf
Pop-Location
$kernel = Join-Path $rootDir "kernel-riscv64\target\riscv64gc-unknown-none-elf\debug\kernel-riscv64"
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemu = Resolve-QemuPath "qemu-system-riscv64.exe"
$logPath = Join-Path $rootDir "target\riscv_boot.log"

if (Test-Path $logPath) { Remove-Item $logPath -Force }

Write-Host "Booting InterOS RISC-V in QEMU virt..." -ForegroundColor Cyan
$p = Start-Process -FilePath $qemu -ArgumentList "-machine virt -cpu rv64 -m 256M -nographic -serial file:`"$logPath`" -bios default -kernel `"$kernel`"" -PassThru

Start-Sleep -Seconds 5
if (-not $p.HasExited) {
    $p.Kill()
}

if (Test-Path $logPath) {
    $lines = Get-Content $logPath
    Write-Host "RISC-V Serial output ($($lines.Count) lines):" -ForegroundColor Green
    $lines | Select-Object -Last 30
} else {
    Write-Warning "No log found."
}
