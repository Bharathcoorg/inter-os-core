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
. (Join-Path $rootDir "tools\qemu_env.ps1")
. (Join-Path $rootDir "tools\build_env.ps1")
$qemu_x86 = Resolve-QemuPath "qemu-system-x86_64.exe"
$qemu_riscv = Resolve-QemuPath "qemu-system-riscv64.exe"
$qemu_aarch64 = Resolve-QemuPath "qemu-system-aarch64.exe"

function Write-Banner {
    param([string]$msg)
    Write-Host ""
    Write-Host "--- InterOS --- $msg" -ForegroundColor Cyan
    Write-Host ""
}

function Invoke-Check {
    Write-Banner "cargo check (fast compile verification)"
    Push-Location $kernelDir
    cargo check --target x86_64-unknown-none
    if ($LASTEXITCODE -ne 0) { throw "cargo check failed" }
    Pop-Location
    Write-Host "[OK] Kernel compiles cleanly." -ForegroundColor Green
}

function Invoke-Build {
    Write-Banner "Building x86_64 kernel + bootloader"

    # WHY userspace is built before the kernel, but only when it is not already
    # correct: the kernel embeds these binaries with include_bytes! from
    # target/x86_64-unknown-none/debug, and the guest ELF loader accepts only
    # ET_EXEC. A plain `cargo build` of those crates writes an ET_DYN binary to
    # that same path, because the *-unknown-none targets default to
    # position-independent executables. Packaging without checking therefore
    # embeds whatever happens to be sitting there, and the device halts at boot
    # with "ELF must be ET_EXEC". tools/build_userspace.ps1 passes -no-pie and
    # asserts ET_EXEC after every binary.
    #
    # WHY it is conditional: rebuilding unconditionally rewrites all 26 binaries
    # on every run, which changes the kernel's embedded inputs and forces a full
    # kernel recompile. That defeats the no-op rebuild property a second
    # consecutive build is supposed to have. The guard below skips the rebuild
    # only when every embedded image is already present, already ET_EXEC, free of
    # the symbol table that makes an image non-reproducible, and no older than the
    # sources it is built from.
    #
    # WHY freshness is part of the guard: being a valid ET_EXEC says nothing about
    # which source an image was built from. Editing an app and rebuilding used to
    # leave the previous binary sitting in target/, package it into the image, and
    # report success, so a benchmark could measure code that was no longer in the
    # tree. Compare against the newest userspace source instead of assuming.
    $embeddedDir = Join-Path $targetDir "x86_64-unknown-none\debug"
    $embeddedImages = @(
        "init", "displayd", "inputd", "storaged", "netd", "powerd", "modemd",
        "audiod", "camerad", "sensord", "compositor", "launcher", "settings",
        "terminal", "dialer", "vault", "camera", "calculator", "browser",
        "messages", "calendar", "music", "file_manager", "gallery",
        "notes"
    )

    $needsUserspaceBuild = $false
    $badImages = @()
    # Resolved once, not per image: the walk touches every source file and the
    # answer is the same for all 25 images.
    $newestSourceUtc = Get-NewestUserspaceSourceUtc -Root $rootDir
    foreach ($image in $embeddedImages) {
        $imagePath = Join-Path $embeddedDir $image
        if (-not (Test-Path -LiteralPath $imagePath)) {
            $badImages += "$image (missing)"
            continue
        }
        $header = [System.IO.File]::ReadAllBytes($imagePath)
        if ($header.Length -lt 18 -or $header[0] -ne 0x7F -or $header[1] -ne 0x45 -or
            $header[2] -ne 0x4C -or $header[3] -ne 0x46) {
            $badImages += "$image (not an ELF)"
            continue
        }
        $eType = [BitConverter]::ToUInt16($header, 16)
        if ($eType -ne 2) {
            $badImages += "$image (ET_DYN, the guest loader requires ET_EXEC)"
            continue
        }
        # A binary that still carries .symtab is not acceptable either, even
        # though it is loadable: LLVM's internal-linkage symbol names embed a
        # hash of the absolute source path, so those tables make the packaged
        # image differ per build directory. Treat them the same as a wrong ELF
        # type so a stale symbol-carrying artifact cannot survive a rebuild.
        try {
            Assert-NoSymbolTable -Path $imagePath -Label $image
        } catch {
            $badImages += "$image (still carries a symbol table, so the image would not be reproducible)"
            continue
        }
        if ($newestSourceUtc -ne $null) {
            $item = Get-Item -LiteralPath $imagePath
            if ($item.LastWriteTimeUtc -lt $newestSourceUtc) {
                $badImages += "$image (older than the newest userspace source at $($newestSourceUtc.ToString('o')); the image predates an edit and would package the previous build)"
            }
        }
    }
    if ($badImages.Count -gt 0) {
        $needsUserspaceBuild = $true
        Write-Host "Ring-3 userspace needs rebuilding:" -ForegroundColor Yellow
        $badImages | ForEach-Object { Write-Host "  - $_" -ForegroundColor Yellow }
    }

    if ($needsUserspaceBuild) {
        $userspaceBuilder = Join-Path $rootDir "tools\build_userspace.ps1"
        if (-not (Test-Path $userspaceBuilder)) {
            throw "Missing build tool: userspace builder not found at $userspaceBuilder"
        }
        $savedDir = $PWD.Path
        try {
            & $userspaceBuilder
            if ($LASTEXITCODE -ne 0) {
                throw "Userspace build failed with exit code $LASTEXITCODE"
            }
        } finally {
            Set-Location $savedDir
        }
    } else {
        Write-Host "[OK] Ring-3 userspace already present, ET_EXEC, symbol-free and no older than its sources; skipping rebuild." -ForegroundColor Green
    }

    $kernelManifest = Join-Path $kernelDir "Cargo.toml"
    $kernelBin = Invoke-CargoBuildArtifact -CargoArgs @("build", "--manifest-path", $kernelManifest, "--target", "x86_64-unknown-none") -TargetName "kernel" -ExpectedKind "bin"

    # Package bootloader disk image
    if (-not (Test-Path $targetDir)) {
        New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
    }
    $biosImg = Join-Path $targetDir "inter-os-bios.img"
    $bootManifest = Join-Path $rootDir "boot\Cargo.toml"
    if (-not (Test-Path $bootManifest)) {
        throw "Missing build tool: Bootloader runner manifest not found at $bootManifest"
    }

    Write-Host "Packaging bootloader image: $biosImg" -ForegroundColor Yellow
    cargo run --manifest-path $bootManifest -- "$kernelBin" "$biosImg"
    if ($LASTEXITCODE -ne 0) {
        throw "BIOS image creation failed with exit code $LASTEXITCODE"
    }

    $verifiedBios = Verify-BiosImageArtifact -ImagePath $biosImg -KernelBinaryPath $kernelBin
    return $verifiedBios
}

function Invoke-Run {
    Write-Banner "Build + Run in QEMU x86_64"
    $biosImg = Invoke-Build

    if (-not (Test-Path $biosImg)) {
        throw "Missing build artifact: BIOS image not found at $biosImg"
    }

    $serialLog = Join-Path $targetDir "serial.log"
    Write-Host "Launching QEMU x86_64 with serial output to: $serialLog" -ForegroundColor Yellow

    & $qemu_x86 `
        -machine q35 `
        -cpu qemu64 `
        -m 1024M `
        -drive "format=raw,file=$biosImg" `
        -serial "file:$serialLog" `
        -monitor "tcp:127.0.0.1:44447,server,nowait" `
        -no-reboot
    if ($LASTEXITCODE -ne 0) {
        throw "QEMU execution exited with code $LASTEXITCODE"
    }
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
        if ($LASTEXITCODE -ne 0) { throw "RISC-V boot test failed with exit code $LASTEXITCODE" }
    }

    # 4. x86_64 boot test
    $x86Test = Join-Path $rootDir "tests\verify_x86_boot.ps1"
    if (Test-Path $x86Test) {
        Write-Host "Running x86_64 boot test..." -ForegroundColor Yellow
        & $x86Test
        if ($LASTEXITCODE -ne 0) { throw "x86_64 boot test failed with exit code $LASTEXITCODE" }
    }

    Write-Host ""
    Write-Host "[OK] All tests completed." -ForegroundColor Green
}

function Invoke-Clean {
    Write-Banner "Cleaning build artifacts"
    Push-Location $kernelDir
    cargo clean
    Pop-Location
    if (Test-Path (Join-Path $rootDir "Cargo.toml")) {
        Push-Location $rootDir
        cargo clean
        Pop-Location
    }
    $biosImg = Join-Path $targetDir "inter-os-bios.img"
    if (Test-Path $biosImg) {
        Remove-Item $biosImg -Force -ErrorAction SilentlyContinue
    }
    Write-Host "[OK] Build artifacts cleaned." -ForegroundColor Green
}

function Invoke-Riscv {
    Write-Banner "Building + Testing RISC-V 64 kernel"
    $rvDir = Join-Path $rootDir "kernel-riscv64"
    if (Test-Path $rvDir) {
        Push-Location $rvDir
        cargo build --target riscv64gc-unknown-none-elf
        $rvExit = $LASTEXITCODE
        Pop-Location
        if ($rvExit -ne 0) { throw "RISC-V build failed with exit code $rvExit" }

        $rvTest = Join-Path $rootDir "tests\verify_riscv_boot.ps1"
        if (Test-Path $rvTest) {
            & $rvTest
            if ($LASTEXITCODE -ne 0) { throw "RISC-V boot verification failed with exit code $LASTEXITCODE" }
        }
    } else {
        throw "kernel-riscv64 directory not found"
    }
}

function Invoke-Aarch64 {
    Write-Banner "Building + Testing AArch64 kernel"
    $aDir = Join-Path $rootDir "kernel-aarch64"
    if (Test-Path $aDir) {
        Push-Location $aDir
        cargo build --target aarch64-unknown-none
        $aExit = $LASTEXITCODE
        Pop-Location
        if ($aExit -ne 0) { throw "AArch64 build failed with exit code $aExit" }

        $aTest = Join-Path $rootDir "tests\test_aarch64.ps1"
        if (Test-Path $aTest) {
            & $aTest
            if ($LASTEXITCODE -ne 0) { throw "AArch64 test failed with exit code $LASTEXITCODE" }
        }
    } else {
        throw "kernel-aarch64 directory not found"
    }
}

function Show-Help {
    Write-Host ""
    Write-Host "InterOS Development CLI" -ForegroundColor Cyan
    Write-Host "-----------------------" -ForegroundColor Cyan
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
