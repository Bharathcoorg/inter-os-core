Stop-Process -Name "qemu-system-riscv64" -Force -ErrorAction SilentlyContinue
$log = "$PSScriptRoot\..\target\riscv64_boot.log"
$k1 = "$PSScriptRoot\..\target\riscv64gc-unknown-none-elf\debug\kernel-riscv64"
$k2 = "$PSScriptRoot\..\kernel-riscv64\target\riscv64gc-unknown-none-elf\debug\kernel-riscv64"
$rootDir = (Resolve-Path "$PSScriptRoot\..").Path
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemuBin = Resolve-QemuPath "qemu-system-riscv64.exe"
$p = Start-Process -FilePath $qemuBin -ArgumentList "-M virt -cpu rv64 -m 1024M -bios default -kernel `"$kernel`" -serial file:`"$log`" -display none" -PassThru
Start-Sleep -Seconds 4
if (-not $p.HasExited) {
    $p.Kill()
}
if (Test-Path $log) {
    Get-Content $log
} else {
    Write-Error "Log file not found!"
}
