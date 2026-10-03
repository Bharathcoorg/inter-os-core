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
Write-Host "[OK] Android Boot Image v2 container header verified (Magic: 'ANDROID!')." -ForegroundColor Green

# --- Payload integrity -------------------------------------------------------
#
# WHY this block exists: validating the 'ANDROID!' magic alone certified
# containers that carried no usable payload at all. The boot image generator used
# to synthesise a "device tree" as the FDT magic 0xd00dfeed followed by the ASCII
# text SOVEREIGN_DEVICE_TREE_BLOB_MOBILE_DEVBOARD_V1, and a "ramdisk" as the CPIO
# magic 070701 followed by ASCII naming init/terminal/dialer. Both start with the
# correct magic, so a magic-only check passed them and then printed
# "[SUCCESS] ... pipeline verified". The header's dtb_size/ramdisk_size were
# non-zero, so nothing downstream could tell the difference. Checking the actual
# leading bytes of each payload catches exactly that class of fabrication.

$pageSize = [System.BitConverter]::ToUInt32($imgBytes, 36)
$kernelSize = [System.BitConverter]::ToUInt32($imgBytes, 8)
$ramdiskSize = [System.BitConverter]::ToUInt32($imgBytes, 16)
$secondSize = [System.BitConverter]::ToUInt32($imgBytes, 24)
$dtbSize = 0
if ($imgBytes.Length -ge 1636) { $dtbSize = [System.BitConverter]::ToUInt32($imgBytes, 1632) }

# WHY these offsets are derived rather than fixed: each section is padded up to a
# page boundary, so the ramdisk does not sit at page 2 unless the kernel happens
# to be exactly one page long. Reading a fixed offset would inspect the wrong
# bytes and reject a genuine image (or accept a fabricated one).
function Get-PageAligned([uint32]$offset, [uint32]$size, [uint32]$page) {
    if ($size -eq 0) { return [uint64]$offset }
    return [uint64]$offset + [uint64]$size + [uint64](($page - ($size % $page)) % $page)
}

$ramdiskOff = [uint64]$pageSize
$secondOff  = Get-PageAligned $ramdiskOff $kernelSize  $pageSize
$ramdiskOff = $secondOff
$dtbOff     = Get-PageAligned $ramdiskOff $ramdiskSize $pageSize
$secondOff  = $dtbOff
$dtbOff     = Get-PageAligned $secondOff $secondSize $pageSize

$payloadProblems = @()

if ($ramdiskSize -eq 0) {
    $payloadProblems += "ramdisk_size is 0 (no initramfs in this image)"
} else {
    if ($ramdiskOff + 6 -gt $imgBytes.Length) {
        $payloadProblems += "ramdisk payload lies past the end of the image"
    } else {
        $ramdiskMagic = [System.Text.Encoding]::ASCII.GetString($imgBytes[$ramdiskOff..($ramdiskOff + 5)])
        if ($ramdiskMagic -ne "070701") {
            $payloadProblems += "ramdisk does not begin with the cpio magic '070701' (found '$ramdiskMagic')"
        }
    }
}

if ($dtbSize -eq 0) {
    $payloadProblems += "dtb_size is 0 (no device tree in this image)"
} else {
    if ($dtbOff + 4 -gt $imgBytes.Length) {
        $payloadProblems += "dtb payload lies past the end of the image"
    } else {
        # WHY structural checks and not just magic: the fabricated payloads both BEGIN
        # with the correct magic (cpio "070701" and FDT "D00DFEED") followed by
        # ASCII text. A magic-prefix test therefore passes them. The newc archive
        # header is 110 bytes of fixed-width ASCII hex and ends with the
        # "TRAILER!!!" record, and an FDT header carries a totalsize plus struct
        # and strings extents. Both placeholders violate those structures, so
        # validating the structure is what actually distinguishes them.

$newcHeaderLen = 110
if ($ramdiskSize -lt ($newcHeaderLen + 10)) {
    $payloadProblems += ("ramdisk is only $ramdiskSize bytes; a cpio newc archive needs at least a $newcHeaderLen-byte header plus a TRAILER!!! record")
} else {
    $trailer = [System.Text.Encoding]::ASCII.GetString($imgBytes[([int]$ramdiskOff + $newcHeaderLen)..([int]$ramdiskOff + $ramdiskSize - 1)])
    if ($trailer -notmatch "TRAILER!!!") {
        $payloadProblems += "ramdisk has no cpio TRAILER!!! record; it is not a real cpio archive"
    }
}

$be32 = {
    param([uint64]$at)
    # WHY multiplication instead of shifts: PowerShell keeps [byte] as the shift
    # operand, so 0xD0 -shl 24 overflows back to 0; and the shifted value becomes
    # a *negative* int (0xD0000000), which PowerShell then refuses to cast to
    # [uint32]. Both faults corrupted every big-endian field, reading a valid
    # FDT magic as 0x000000ED. Scaling by powers of 256 stays unsigned.
    ([uint32]$imgBytes[$at] * 0x1000000) +
    ([uint32]$imgBytes[$at + 1] * 0x10000) +
    ([uint32]$imgBytes[$at + 2] * 0x100) +
    ([uint32]$imgBytes[$at + 3])
}

if ($dtbSize -lt 40) {
    $payloadProblems += ("dtb is only $dtbSize bytes; a flattened device tree needs at least a 40-byte header")
} else {
    $totalSize = & $be32 ([uint64]$dtbOff + 4)
    $offStruct = & $be32 ([uint64]$dtbOff + 8)
    $offStrings = & $be32 ([uint64]$dtbOff + 12)
    # WHY offsets 32 and 36: the flattened device tree header layout is
    # magic(0) totalsize(4) off_dt_struct(8) off_dt_strings(12) off_mem_rsvmap(16)
    # version(20) last_comp_version(24) boot_cpuid_phys(28) size_dt_strings(32)
    # size_dt_struct(36) size_dt_boot(40). Reading 20/24 yields the version
    # fields and rejects every genuine device tree.
    $sizeStrings = & $be32 ([uint64]$dtbOff + 32)
    $sizeStruct = & $be32 ([uint64]$dtbOff + 36)

    if ($totalSize -lt 40 -or $totalSize -gt $dtbSize) {
        $payloadProblems += "dtb header totalsize $totalSize is not within [40, $dtbSize]"
    }
    if (($offStruct + $sizeStruct) -gt $dtbSize -or ($offStrings + $sizeStrings) -gt $dtbSize) {
        $payloadProblems += "dtb struct/strings extents extend past the payload"
    }
    if ($totalSize -ge 40 -and $offStruct -ge 40 -and $offStruct -lt $totalSize) {
        # The struct block must open with an FDT_BEGIN (0x00000001) node token.
        $firstToken = & $be32 ([uint64]$dtbOff + $offStruct)
        if ($firstToken -ne 0x00000001) {
            $payloadProblems += ("dtb struct block does not open with an FDT_BEGIN token (found 0x{0:X8})" -f $firstToken)
        }
    } elseif ($totalSize -ge 40) {
        $payloadProblems += "dtb off_dt_struct $offStruct is not inside the payload"
    }
}
    }
}

if ($payloadProblems.Count -gt 0) {
    Write-Host ""
    Write-Host "[REJECTED] This image is a valid container but its payloads are not real:" -ForegroundColor Red
    foreach ($p in $payloadProblems) { Write-Host "          - $p" -ForegroundColor Red }
    Write-Host ""
    Write-Host "[REJECTED] Refusing to report this image as verified or to flash it." -ForegroundColor Red
    Write-Host "[REJECTED] A device that received this image would not boot, and the container" -ForegroundColor Red
    Write-Host "[REJECTED] would have flashed without complaint." -ForegroundColor Red
    exit 1
}

Write-Host "[OK] Payload integrity verified: ramdisk carries a cpio archive, dtb carries an FDT." -ForegroundColor Green
Write-Host ""
Write-Host "[NOT READY] This platform has never been booted on physical hardware." -ForegroundColor Yellow
Write-Host "[NOT READY] The AArch64 kernel has no display, touch, or phone storage driver," -ForegroundColor Yellow
Write-Host "[NOT READY] so a successful flash is not expected to reach a usable interface." -ForegroundColor Yellow

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
    Write-Host "[INFO] Container structure and payload integrity verified." -ForegroundColor Green
    Write-Host "[INFO] Readiness for real hardware is NOT established -- see the [NOT READY] notices above." -ForegroundColor Yellow
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
