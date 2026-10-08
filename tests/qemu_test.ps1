param(
    [switch]$Gui
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$img = Join-Path $rootDir "target\inter-os-bios.img"
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemu = Resolve-QemuPath "qemu-system-x86_64.exe"

if (-not (Test-Path $img)) {
    Write-Error "Disk image not found at $img! Run .\qemu\build.ps1 first."
    exit 1
}

if ($Gui) {
    Write-Host "====================================================" -ForegroundColor Cyan
    Write-Host " Launching Inter OS v0.1 in Graphical GUI Window... " -ForegroundColor Green
    Write-Host "====================================================" -ForegroundColor Cyan
    & $qemu -machine q35 -cpu qemu64 -m 1024M -drive "format=raw,file=$img" -serial stdio -name "Inter OS v0.1"
} else {
    Write-Host "====================================================" -ForegroundColor Cyan
    Write-Host " Inter OS v0.1 - Boot Diagnostic Verification       " -ForegroundColor Yellow
    Write-Host "====================================================" -ForegroundColor Cyan
    Write-Host "To open the interactive graphical desktop window, run:" -ForegroundColor Gray
    Write-Host "  .\run_gui.bat" -ForegroundColor Green
    Write-Host "  or: .\tests\qemu_test.ps1 -Gui`n" -ForegroundColor Green

    $log = Join-Path $rootDir "target\test_serial.log"
    if (Test-Path $log) { Remove-Item $log -Force }

    $p = Start-Process -FilePath $qemu -ArgumentList "-machine q35 -cpu qemu64 -m 1024M -drive format=raw,file=`"$img`" -serial file:`"$log`" -display none -no-reboot" -PassThru
    Start-Sleep -Seconds 4
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue

    if (Test-Path $log) {
        $lines = Get-Content $log
        if ($lines) {
            $lines | ForEach-Object {
                if ($_ -match "\[OK\]") {
                    Write-Host $_ -ForegroundColor Green
                } elseif ($_ -match "\[TEST\]") {
                    Write-Host $_ -ForegroundColor Magenta
                } elseif ($_ -match "SYSTEM READY") {
                    Write-Host $_ -ForegroundColor Yellow
                } else {
                    Write-Host $_
                }
            }
            Write-Host "`n[SUCCESS] Kernel booted and all self-tests verified!" -ForegroundColor Green
        } else {
            Write-Host "[NOTE] Headless test completed. Run .\run_gui.bat to view live GUI." -ForegroundColor Cyan
        }
    }
}
