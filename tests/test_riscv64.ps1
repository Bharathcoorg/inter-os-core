Stop-Process -Name "qemu-system-riscv64" -Force -ErrorAction SilentlyContinue
$log = "$PSScriptRoot\..\target\riscv64_boot.log"
if (Test-Path $log) { Remove-Item -Force $log }
$k1 = "$PSScriptRoot\..\target\riscv64gc-unknown-none-elf\debug\kernel-riscv64"
$k2 = "$PSScriptRoot\..\kernel-riscv64\target\riscv64gc-unknown-none-elf\debug\kernel-riscv64"
$kernel = if ((Test-Path $k1) -and (Test-Path $k2)) {
    if ((Get-Item $k1).LastWriteTime -ge (Get-Item $k2).LastWriteTime) { $k1 } else { $k2 }
} elseif (Test-Path $k1) { $k1 } else { $k2 }
$rootDir = (Resolve-Path "$PSScriptRoot\..").Path
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemuBin = Resolve-QemuPath "qemu-system-riscv64.exe"
$p = Start-Process -FilePath $qemuBin -ArgumentList "-M virt -cpu rv64 -m 1024M -bios default -kernel `"$kernel`" -serial file:`"$log`" -display none" -PassThru
Start-Sleep -Seconds 7
if (-not $p.HasExited) {
    $p.Kill()
}
$content = if (Test-Path $log) { Get-Content $log } else { @() }
Write-Host "=== Output Preview (Last 50 lines) ===" -ForegroundColor Cyan
$content | Select-Object -Last 50

$requiredMilestones = @(
    "OpenSBI",
    "Inter OS",
    "MMU: Sv39 3-level page tables active",
    "Traps: stvec vector table online",
    "RISC-V Security Invariant: Kernel address pointer rejected",
    "RISC-V Userspace Syscall: SYS_GET_TASK_ID returned valid Task ID",
    "System state: Userspace Ready",
    "Dropping CPU execution privilege from S-mode to U-mode via sret",
    "Hello from InterOS RISC-V Userspace!",
    "Task exited with code 0"
)

$missing = @()
$rawText = ($content -join "`n")
foreach ($m in $requiredMilestones) {
    if (-not ($rawText -match [regex]::Escape($m))) {
        $missing += $m
    }
}

if ($missing.Count -gt 0) {
    Write-Error "RISC-V64 boot verification FAILED: Missing milestones: $($missing -join ', ')"
    exit 1
}

Write-Host "[OK] RISC-V64 QEMU virt platform execution verified for tested OpenSBI, Sv39 MMU, trap, and U-mode syscall invariants." -ForegroundColor Green
exit 0
