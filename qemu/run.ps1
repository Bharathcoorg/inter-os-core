param(
    [switch]$NoDisplay = $false,
    [int]$TimeoutSeconds = 0
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$imagePath = "$rootDir\target\inter-os-bios.img"
$qemuBin = "C:\Users\bhara\qemu\qemu-system-x86_64.exe"

if (-not (Test-Path $imagePath)) {
    Write-Host "Disk image not found. Running build.ps1 first..." -ForegroundColor Yellow
    & "$scriptDir\build.ps1"
}

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  Booting Gravity OS v0.1 in QEMU" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "Image: $imagePath"
Write-Host "QEMU:  $qemuBin"

$qemuArgs = @(
    "-drive", "format=raw,file=$imagePath",
    "-m", "512M",
    "-serial", "stdio",
    "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"
)

if ($NoDisplay) {
    $qemuArgs += @("-display", "none")
}

& $qemuBin @qemuArgs
