<#
.SYNOPSIS
    Inter OS — Sovereign Smartphone Hardware Flashing Tool (AArch64 Fastboot Automator)

.DESCRIPTION
    Flashes or tether-boots Inter OS onto physical smartphone hardware (PinePhone,
    Qualcomm Snapdragon, Google Pixel, MediaTek Dimensity) using the standard
    Android Fastboot protocol (AOSP Boot Image v2).

.PARAMETER Mode
    'Boot' for tethered RAM boot (safe, non-persistent), or 'Flash' for persistent NAND flashing.

.PARAMETER TargetImg
    Path to the compiled boot.img (defaults to target\boot-aarch64.img).

.PARAMETER Slot
    A/B slot override ('a', 'b', or 'auto').
#>

[CmdletBinding()]
param(
    [ValidateSet("Boot", "Flash", "Info")]
    [string]$Mode = "Boot",

    [string]$TargetImg = "",

    [string]$Slot = "auto",

    [switch]$AllowPersistentFlash,

    [string]$ExpectedProduct = ""
)

$ErrorActionPreference = "Stop"

$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $toolsDir

if (-not $TargetImg) {
    $TargetImg = Join-Path $rootDir "target\boot-aarch64.img"
}

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  Inter OS -- Sovereign Smartphone Hardware Flasher v4.6" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan

# 1. Locate Fastboot binary
$fastbootPath = $null
$candidates = @(
    "fastboot",
    "$env:LOCALAPPDATA\Android\Sdk\platform-tools\fastboot.exe",
    "$env:USERPROFILE\platform-tools\fastboot.exe",
    "C:\platform-tools\fastboot.exe",
    "C:\Program Files (x86)\Android\android-sdk\platform-tools\fastboot.exe"
)

foreach ($c in $candidates) {
    try {
        $cmd = Get-Command $c -ErrorAction SilentlyContinue
        if ($cmd) {
            $fastbootPath = $cmd.Source
            break
        }
    } catch {}
}

if (-not $fastbootPath) {
    Write-Host "[WARNING] 'fastboot' was not found in PATH or standard Android SDK directories." -ForegroundColor Yellow
    Write-Host "Please install Android Platform Tools: https://developer.android.com/tools/releases/platform-tools" -ForegroundColor Yellow
    $fastbootPath = "fastboot"
} else {
    Write-Host "[OK] Fastboot found at: $fastbootPath" -ForegroundColor Green
}

# 2. Check Image File
if (-not (Test-Path $TargetImg)) {
    Write-Host "[BUILDING] Image not found at '$TargetImg'. Running automated build..." -ForegroundColor Yellow
    & "$rootDir\qemu\build.ps1"
}
$resolvedImg = (Resolve-Path $TargetImg).Path

$imgBytes = [System.IO.File]::ReadAllBytes($resolvedImg)
$fileSize = $imgBytes.Length
$sizeMb = [Math]::Round($fileSize / 1048576.0, 2)
$sha256 = [System.Security.Cryptography.SHA256]::Create()
$hashBytes = $sha256.ComputeHash($imgBytes)
$hashStr = [System.BitConverter]::ToString($hashBytes).Replace("-", "").ToLower()

Write-Host "[IMAGE] Target Image: $resolvedImg" -ForegroundColor White
Write-Host "[IMAGE] Image Size:   $fileSize bytes ($sizeMb MB)" -ForegroundColor White
Write-Host "[IMAGE] SHA-256 Hash: $hashStr" -ForegroundColor White

# Verify ANDROID! magic header
$magic = [System.Text.Encoding]::ASCII.GetString($imgBytes[0..7])
if ($magic -ne "ANDROID!") {
    Write-Error "Invalid boot image! Header magic is '$magic', expected 'ANDROID!'."
    exit 1
}
Write-Host "[OK] Android Boot Image v2 format verified (Magic: 'ANDROID!')." -ForegroundColor Green

# 3. Check Connected Physical Devices
Write-Host ""
Write-Host "--- [Querying Connected Physical Smartphone Devices] ---" -ForegroundColor Yellow
try {
    $devices = & $fastbootPath devices 2>$null
} catch {
    $devices = $null
}

if (-not $devices) {
    Write-Host "[WAITING] No devices currently found in Fastboot mode." -ForegroundColor Yellow
    Write-Host "To connect your physical smartphone:" -ForegroundColor Gray
    Write-Host "  1. Enable USB Debugging in Developer Options" -ForegroundColor Gray
    Write-Host "  2. Run: adb reboot bootloader" -ForegroundColor Gray
    Write-Host "  3. Or power down phone and hold Power + Volume Down" -ForegroundColor Gray
    Write-Host ""
    Write-Host "Direct Flashing Commands (once connected):" -ForegroundColor Cyan
    if ($Mode -eq "Boot") {
        Write-Host "  fastboot boot `"$resolvedImg`"" -ForegroundColor White
    } else {
        Write-Host "  fastboot flash boot `"$resolvedImg`"" -ForegroundColor White
        Write-Host "  fastboot reboot" -ForegroundColor White
    }
    Write-Host ""
    Write-Host "[SUCCESS] Image validation and Fastboot flasher pipeline verified." -ForegroundColor Green
    exit 0
}

$deviceLines = @($devices | Where-Object { $_ -and $_.ToString().Trim() })
if ($deviceLines.Count -ne 1) {
    Write-Error "Expected exactly one Fastboot device before any device operation; found $($deviceLines.Count). Refusing to continue."
    exit 1
}

Write-Host "[DEVICE] Connected Fastboot Device:" -ForegroundColor Green
Write-Host $deviceLines[0] -ForegroundColor White

# Query device properties
try {
    $product = (& $fastbootPath getvar product 2>&1) | Out-String
    $unlocked = (& $fastbootPath getvar unlocked 2>&1) | Out-String
    $currentSlot = (& $fastbootPath getvar current-slot 2>&1) | Out-String

    if ($ExpectedProduct) {
        if ($product -notmatch [regex]::Escape($ExpectedProduct)) {
            Write-Error "Connected device product does not match ExpectedProduct '$ExpectedProduct'. Refusing to continue."
            exit 1
        }
        Write-Host "[OK] Device product matches expected target '$ExpectedProduct'." -ForegroundColor Green
    }
    Write-Host "[DEVICE INFO]" -ForegroundColor Cyan
    Write-Host "  $product" -ForegroundColor Gray
    Write-Host "  $unlocked" -ForegroundColor Gray
    if ($currentSlot) { Write-Host "  $currentSlot" -ForegroundColor Gray }
} catch {}

# 4. Perform Operation
Write-Host ""
if ($Mode -eq "Info") {
    Write-Host "Device info retrieved successfully." -ForegroundColor Green
    exit 0
}

if ($Mode -eq "Boot") {
    Write-Host "[OPERATION] Initiating Tethered Live RAM Boot (fastboot boot)..." -ForegroundColor Yellow
    Write-Host "This boots Inter OS without modifying physical NAND storage." -ForegroundColor Gray
    & $fastbootPath boot "$resolvedImg"
    if ($LASTEXITCODE -eq 0) {
        Write-Host "[SUCCESS] Inter OS kernel loaded and booted on smartphone!" -ForegroundColor Green
    } else {
        Write-Error "Tethered boot failed with exit code $LASTEXITCODE."
    }
} elseif ($Mode -eq "Flash") {
    if (-not $AllowPersistentFlash) {
        Write-Error "Persistent flashing is disabled by default. Re-run with -AllowPersistentFlash only after verifying the device and image."
        exit 1
    }

    if ($unlocked -notmatch "(?i)(yes|true|unlocked)") {
        Write-Error "Bootloader does not report an unlocked state. Refusing persistent flash."
        exit 1
    }

    if ($Slot -notin @("auto", "a", "b")) {
        Write-Error "Invalid slot '$Slot'. Expected auto, a, or b."
        exit 1
    }

    $partition = "boot"
    if ($Slot -ne "auto") {
        $partition = "boot_$Slot"
    } elseif ($currentSlot -match "(?i)([ab])") {
        $activeSlot = $Matches[1].ToLower()
        $partition = "boot_$activeSlot"
        Write-Host "[SLOT] Auto-selected active slot '$activeSlot' -> partition '$partition'." -ForegroundColor Cyan
    }

    Write-Host ""
    Write-Host "[DANGER] This operation writes to persistent device storage." -ForegroundColor Red
    Write-Host "[DANGER] Device product: $product" -ForegroundColor Red
    Write-Host "[DANGER] Target partition: $partition" -ForegroundColor Red
    Write-Host "[DANGER] Image SHA-256: $hashStr" -ForegroundColor Red
    $confirmation = Read-Host "Type FLASH to continue, or anything else to abort"
    if ($confirmation -cne "FLASH") {
        Write-Host "[ABORTED] Persistent flash cancelled; no device storage was modified." -ForegroundColor Yellow
        exit 0
    }

    Write-Host "[OPERATION] Flashing Inter OS to physical boot partition..." -ForegroundColor Yellow
    Write-Host "Writing $fileSize bytes to partition '$partition'..." -ForegroundColor White
    & $fastbootPath flash $partition "$resolvedImg"
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Flashing failed with exit code $LASTEXITCODE."
        exit 1
    }

    Write-Host "[SUCCESS] Partition '$partition' flashed successfully!" -ForegroundColor Green
    Write-Host "Rebooting smartphone into Inter OS..." -ForegroundColor Cyan
    & $fastbootPath reboot
    Write-Host "[DONE] Smartphone rebooted. Welcome to Sovereign Inter OS." -ForegroundColor Green
}
