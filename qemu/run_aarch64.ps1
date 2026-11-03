# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

param(
    [switch]$NoDisplay = $false,
    [int]$TimeoutSeconds = 0
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$kernelPath = "$rootDir\target\aarch64-unknown-none\debug\kernel-aarch64"
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemuBin = Resolve-QemuPath "qemu-system-aarch64.exe"

if (-not (Test-Path $kernelPath)) {
    Write-Host "AArch64 kernel binary not found. Compiling now..." -ForegroundColor Yellow
    cargo +nightly rustc --manifest-path "$rootDir\kernel-aarch64\Cargo.toml" --target aarch64-unknown-none -- -C link-arg=-Tkernel-aarch64/linker.ld
}

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  Booting Inter OS (AArch64 Smartphone Core) in QEMU" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "Kernel: $kernelPath"
Write-Host "Target: ARM Cortex-A53 (InterOS Reference Mobile Hardware / QEMU Virt)"
Write-Host "QEMU:   $qemuBin"

$qemuArgs = @(
    "-M", "virt",
    "-cpu", "cortex-a53",
    "-m", "1024M",
    "-kernel", $kernelPath,
    "-serial", "stdio"
)

if ($NoDisplay) {
    $qemuArgs += @("-display", "none")
}

& $qemuBin @qemuArgs
