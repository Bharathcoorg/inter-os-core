Stop-Process -Name "qemu-system-aarch64" -Force -ErrorAction SilentlyContinue
$log = "$PSScriptRoot\..\target\test_aarch64_v6_1.log"
$k1 = "$PSScriptRoot\..\target\aarch64-unknown-none\debug\kernel-aarch64"
$k2 = "$PSScriptRoot\..\kernel-aarch64\target\aarch64-unknown-none\debug\kernel-aarch64"
$kernel = if ((Test-Path $k2) -and (-not (Test-Path $k1) -or (Get-Item $k2).LastWriteTime -gt (Get-Item $k1).LastWriteTime)) { $k2 } else { $k1 }
$p = Start-Process -FilePath "C:\Users\bhara\qemu\qemu-system-aarch64.exe" -ArgumentList "-M virt -cpu cortex-a53 -m 1024M -kernel `"$kernel`" -serial file:`"$log`" -display none" -PassThru
Start-Sleep -Seconds 15
if (-not $p.HasExited) {
    $p.Kill()
}
$content = Get-Content $log
Write-Host "=== Output Preview (Last 50 lines) ===" -ForegroundColor Cyan
$content | Select-Object -Last 50
