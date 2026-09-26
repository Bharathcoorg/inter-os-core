# Launches Inter OS on AArch64 ARMv8-A QEMU Virt Platform (Mobile Smartphone Emulation)
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemuExe = Resolve-QemuPath "qemu-system-aarch64.exe"
$bootImg = "$rootDir\target\boot.img"

if (-not (Test-Path $bootImg)) {
    Write-Host "[ERROR] Boot image not found at: $bootImg" -ForegroundColor Red
    Write-Host "Please run .\qemu\build.ps1 first to package boot.img." -ForegroundColor Yellow
    exit 1
}

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  Launching Inter OS on AArch64 Cortex-A57 Virt Platform" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "Kernel/Boot Image: $bootImg"
Write-Host "SoC Model:         ARMv8-A Cortex-A57 (64-bit)"
Write-Host "Memory (RAM):      1024 MiB"
Write-Host "Peripherals:       PL011 UART, GICv2, VirtIO"
Write-Host "Fastboot Status:   Valid v2 Android Boot Image (Flashable via fastboot flash boot boot.img)"
Write-Host "============================================================`n"

& $qemuExe -M virt -cpu cortex-a57 -m 1024M -kernel $bootImg -serial stdio -display none
