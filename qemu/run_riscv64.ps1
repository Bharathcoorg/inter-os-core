param(
    [switch]$NoDisplay = $false,
    [int]$TimeoutSeconds = 0
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$kernelPath = "$rootDir\target\riscv64gc-unknown-none-elf\debug\kernel-riscv64"
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemuBin = Resolve-QemuPath "qemu-system-riscv64.exe"

if (-not (Test-Path $kernelPath)) {
    Write-Host "RISC-V 64 kernel binary not found. Compiling now..." -ForegroundColor Yellow
    cargo rustc --manifest-path "$rootDir\kernel-riscv64\Cargo.toml" --target riscv64gc-unknown-none-elf -- -C link-arg=-Tkernel-riscv64/linker.ld
}

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  Booting Inter OS (RISC-V 64 Sovereign Core) in QEMU" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "Kernel: $kernelPath"
Write-Host "Target: RISC-V 64 GC (InterOS Reference Mobile Hardware / QEMU Virt)"
Write-Host "QEMU:   $qemuBin"

$qemuArgs = @(
    "-M", "virt",
    "-cpu", "rv64",
    "-m", "1024M",
    "-bios", "default",
    "-kernel", $kernelPath,
    "-serial", "stdio"
)

if ($NoDisplay) {
    $qemuArgs += @("-display", "none")
}

& $qemuBin @qemuArgs
