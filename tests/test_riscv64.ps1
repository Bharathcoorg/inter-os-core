Stop-Process -Name "qemu-system-riscv64" -Force -ErrorAction SilentlyContinue
$log = "$PSScriptRoot\..\target\riscv64_boot.log"
if (Test-Path $log) { Remove-Item $log -Force }
$kernel = "$PSScriptRoot\..\target\riscv64gc-unknown-none-elf\debug\kernel-riscv64"
$p = Start-Process -FilePath "C:\Users\bhara\qemu\qemu-system-riscv64.exe" -ArgumentList "-M virt -cpu rv64 -m 1024M -bios default -kernel `"$kernel`" -serial file:`"$log`" -display none" -PassThru
Start-Sleep -Seconds 4
if (-not $p.HasExited) {
    $p.Kill()
}
if (Test-Path $log) {
    Get-Content $log
} else {
    Write-Error "Log file not found!"
}
