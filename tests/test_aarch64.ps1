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
$content = if (Test-Path $log) { Get-Content $log } else { @() }
Write-Host "=== Output Preview (Last 50 lines) ===" -ForegroundColor Cyan
$content | Select-Object -Last 50

$requiredMilestones = @(
    "SimpleFB: 720x1440 RGB565 mobile display splash rasterized",
    "SDHCI / eMMC 5.1",
    "Goodix GT911",
    "AArch64 Context Switch",
    "AArch64 Multi-Tasking Scheduler",
    "AArch64 Capability-Guarded IPC",
    "AArch64 Vector Table (VBAR_EL1)",
    "AArch64 MMU",
    "AArch64 EL0 Userspace",
    "AArch64 Sovereign Smartphone Microkernel"
)

$missing = @()
$rawText = ($content -join "`n")
foreach ($m in $requiredMilestones) {
    if (-not ($rawText -match [regex]::Escape($m))) {
        $missing += $m
    }
}

if ($missing.Count -gt 0) {
    Write-Error "AArch64 boot verification FAILED: Missing milestones: $($missing -join ', ')"
    exit 1
}

Write-Host "[OK] All AArch64 hardware drivers, security, and runtime milestones verified!" -ForegroundColor Green
exit 0
