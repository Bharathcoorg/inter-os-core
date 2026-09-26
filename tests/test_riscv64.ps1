Stop-Process -Name "qemu-system-riscv64" -Force -ErrorAction SilentlyContinue
$log = "$PSScriptRoot\..\target\riscv64_boot.log"
$k1 = "$PSScriptRoot\..\target\riscv64gc-unknown-none-elf\debug\kernel-riscv64"
$k2 = "$PSScriptRoot\..\kernel-riscv64\target\riscv64gc-unknown-none-elf\debug\kernel-riscv64"
$kernel = if ((Test-Path $k2) -and (-not (Test-Path $k1) -or (Get-Item $k2).LastWriteTime -gt (Get-Item $k1).LastWriteTime)) { $k2 } else { $k1 }
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
