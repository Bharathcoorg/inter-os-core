param(
    [switch]$NoDisplay = $false,
    [int]$TimeoutSeconds = 0
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$imagePath = "$rootDir\target\inter-os-bios.img"
$dataPath = "$rootDir\target\inter-os-data-x86_64.img"
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemuBin = Resolve-QemuPath "qemu-system-x86_64.exe"

if (-not (Test-Path $imagePath)) {
    Write-Host "Disk image not found. Running build.ps1 first..." -ForegroundColor Yellow
    & "$scriptDir\build.ps1"
}

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  Booting InterOS Core in QEMU" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "Image: $imagePath"
Write-Host "QEMU:  $qemuBin"

$qemuArgs = @(
    "-drive", "format=raw,file=$imagePath,index=0",
    "-drive", "format=raw,file=$dataPath,index=1",
    "-m", "512M",
    "-serial", "stdio",
    "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"
)

# The guest formats the Primary Slave as its filesystem and treats the Primary
# Master as the boot image. Handing it a single disk makes it fall back to
# formatting the boot image, which destroys the bootloader stages the kernel is
# embedded in, so every boot after the first runs a corrupted kernel.
if (-not (Test-Path $dataPath)) {
    $qemuImg = Resolve-QemuPath "qemu-img.exe"
    & $qemuImg create -f raw $dataPath 512M | Out-Null
    Write-Host "Created data disk: $dataPath" -ForegroundColor DarkGray
}

if ($NoDisplay) {
    $qemuArgs += @("-display", "none")
}

& $qemuBin @qemuArgs
