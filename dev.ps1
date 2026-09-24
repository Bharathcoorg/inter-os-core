# InterOS Development CLI — build, run, test, flash
# Usage: .\dev.ps1 <command> [options]
#
# Commands:
#   build       Build the x86_64 kernel + bootloader image
#   run         Build and launch in QEMU (with serial + monitor)
#   test        Run all automated verification tests
#   flash       Build BIOS image for physical/emulated device
#   check       Run cargo check (fast compile-error detection)
#   clean       Clean all build artifacts
#   riscv       Build and test RISC-V 64 kernel in QEMU
#   aarch64     Build and test AArch64 kernel in QEMU

param(
    [Parameter(Position=0)]
    [string]$Command = "help"
)

$ErrorActionPreference = "Stop"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = $scriptDir
$kernelDir = Join-Path $rootDir "kernel"
$targetDir = Join-Path $rootDir "target"
$qemu_x86 = "C:\Users\bhara\qemu\qemu-system-x86_64.exe"
$qemu_riscv = "C:\Users\bhara\qemu\qemu-system-riscv64.exe"
$qemu_aarch64 = "C:\Users\bhara\qemu\qemu-system-aarch64.exe"

function Write-Banner {
    param([string]$msg)
    Write-Host ""
    Write-Host "━━━ InterOS ━━━ $msg" -ForegroundColor Cyan
    Write-Host ""
}

function Invoke-Check {
    Write-Banner "cargo check (fast compile verification)"
    Push-Location $kernelDir
    cargo check
    if ($LASTEXITCODE -ne 0) { throw "cargo check failed" }
    Pop-Location
    Write-Host "[OK] Kernel compiles cleanly." -ForegroundColor Green
}

function Invoke-Build {
    Write-Banner "Building x86_64 kernel + bootloader"
    Push-Location $kernelDir
    cargo build
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    Pop-Location

    # Build bootloader image
    $biosImg = Join-Path $targetDir "inter-os-bios.img"
    $kernelBin = Join-Path $targetDir "x86_64-interos\debug\kernel"
    if (Test-Path $kernelBin) {
        Write-Host "[OK] Kernel binary: $kernelBin" -ForegroundColor Green
    }
    if (Test-Path $biosImg) {
        $sz = (Get-Item $biosImg).Length
        Write-Host "[OK] BIOS image: $biosImg ($sz bytes)" -ForegroundColor Green
    } else {
        Write-Host "[INFO] Run bootloader-runner to create BIOS image" -ForegroundColor Yellow
    }
}

function Invoke-Run {
    Write-Banner "Build + Run in QEMU x86_64"
    Invoke-Build

    $biosImg = Join-Path $targetDir "inter-os-bios.img"
    if (-not (Test-Path $biosImg)) {
        throw "BIOS image not found at $biosImg"
    }

    # Kill any existing QEMU instances
    Get-Process -Name "qemu-system-x86_64" -ErrorAction SilentlyContinue | Stop-Process -Force

    $serialLog = Join-Path $targetDir "serial.log"
    Write-Host "Launching QEMU x86_64 with serial output to: $serialLog" -ForegroundColor Yellow

    & $qemu_x86 `
        -m 1024M `
        -drive "format=raw,file=$biosImg" `
        -serial "file:$serialLog" `
        -monitor "tcp:127.0.0.1:44447,server,nowait" `
        -no-reboot
}

function Invoke-Test {
    Write-Banner "Running automated test suite"

    # 1. Compile check
    Invoke-Check

    # 2. Build
    Invoke-Build

    # 3. RISC-V boot test
    $rvTest = Join-Path $rootDir "tests\verify_riscv_boot.ps1"
    if (Test-Path $rvTest) {
        Write-Host "Running RISC-V boot test..." -ForegroundColor Yellow
        & $rvTest
    }

    # 4. x86_64 boot test
    $x86Test = Join-Path $rootDir "tests\verify_x86_boot.ps1"
    if (Test-Path $x86Test) {
        Write-Host "Running x86_64 boot test..." -ForegroundColor Yellow
        & $x86Test
    }

    Write-Host ""
    Write-Host "[OK] All tests completed." -ForegroundColor Green
}

function Invoke-Clean {
    Write-Banner "Cleaning build artifacts"
    Push-Location $kernelDir
    cargo clean
    Pop-Location
    Write-Host "[OK] Build artifacts cleaned." -ForegroundColor Green
}

function Invoke-Riscv {
    Write-Banner "Building + Testing RISC-V 64 kernel"
    $rvDir = Join-Path $rootDir "kernel-riscv64"
    if (Test-Path $rvDir) {
        Push-Location $rvDir
        cargo build --target riscv64gc-unknown-none-elf
        if ($LASTEXITCODE -ne 0) { throw "RISC-V build failed" }
        Pop-Location

        $rvTest = Join-Path $rootDir "tests\verify_riscv_boot.ps1"
        if (Test-Path $rvTest) {
            & $rvTest
        }
    } else {
        Write-Host "kernel-riscv64 directory not found" -ForegroundColor Red
    }
}

function Invoke-Aarch64 {
    Write-Banner "Building + Testing AArch64 kernel"
    $aDir = Join-Path $rootDir "kernel-aarch64"
    if (Test-Path $aDir) {
        Push-Location $aDir
        cargo build --target aarch64-unknown-none
        if ($LASTEXITCODE -ne 0) { throw "AArch64 build failed" }
        Pop-Location

        $aTest = Join-Path $rootDir "tests\test_aarch64.ps1"
        if (Test-Path $aTest) {
            & $aTest
        }
    } else {
        Write-Host "kernel-aarch64 directory not found" -ForegroundColor Red
    }
}

function Show-Help {
    Write-Host ""
    Write-Host "InterOS Development CLI" -ForegroundColor Cyan
    Write-Host "━━━━━━━━━━━━━━━━━━━━━━━" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Usage: .\dev.ps1 <command>" -ForegroundColor White
    Write-Host ""
    Write-Host "Commands:" -ForegroundColor Yellow
    Write-Host "  build       Build x86_64 kernel + bootloader image"
    Write-Host "  run         Build and launch in QEMU (serial + monitor)"
    Write-Host "  test        Run all automated verification tests"
    Write-Host "  check       Fast compile-error detection"
    Write-Host "  clean       Clean all build artifacts"
    Write-Host "  riscv       Build + test RISC-V 64 kernel"
    Write-Host "  aarch64     Build + test AArch64 kernel"
    Write-Host "  help        Show this help message"
    Write-Host ""
}

switch ($Command.ToLower()) {
    "build"   { Invoke-Build }
    "run"     { Invoke-Run }
    "test"    { Invoke-Test }
    "check"   { Invoke-Check }
    "clean"   { Invoke-Clean }
    "riscv"   { Invoke-Riscv }
    "aarch64" { Invoke-Aarch64 }
    "help"    { Show-Help }
    default   { Show-Help }
}
