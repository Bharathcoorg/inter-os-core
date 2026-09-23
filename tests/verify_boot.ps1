$logFile = "target\boot_run.log"
if (Test-Path $logFile) { Remove-Item $logFile -Force }

$p = Start-Process -FilePath "C:\Users\bhara\qemu\qemu-system-x86_64.exe" -ArgumentList "-drive format=raw,file=target\inter-os-bios.img -serial file:$logFile -display none -no-reboot" -PassThru
Start-Sleep -Seconds 3
Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue

if (Test-Path $logFile) {
    Get-Content $logFile
} else {
    Write-Error "Log file not found!"
}
