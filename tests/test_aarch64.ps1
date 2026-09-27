Stop-Process -Name "qemu-system-aarch64" -Force -ErrorAction SilentlyContinue
$log = "$PSScriptRoot\..\target\test_aarch64_v6_1.log"
$k1 = "$PSScriptRoot\..\target\aarch64-unknown-none\debug\kernel-aarch64"
$k2 = "$PSScriptRoot\..\kernel-aarch64\target\aarch64-unknown-none\debug\kernel-aarch64"
$kernel = if (Test-Path $k1) { $k1 } else { $k2 }
$rootDir = (Resolve-Path "$PSScriptRoot\..").Path
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemuBin = Resolve-QemuPath "qemu-system-aarch64.exe"
$p = Start-Process -FilePath $qemuBin -ArgumentList "-M virt -cpu cortex-a53 -m 1024M -kernel `"$kernel`" -serial file:`"$log`" -display none" -PassThru
Start-Sleep -Seconds 15
if (-not $p.HasExited) {
    $p.Kill()
}
$content = Get-Content $log
Write-Host "=== Output Preview (Last 50 lines) ===" -ForegroundColor Cyan
$content | Select-Object -Last 50
