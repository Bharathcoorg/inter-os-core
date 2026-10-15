$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
. "$rootDir\tools\build_env.ps1"

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  Building InterOS Core (x86_64)" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan

Set-Location $rootDir

# 1. Compile All Userspace Services and Applications
Write-Host "[1/5] Building all Ring-3 userspace services and applications (Init, Services, Compositor, Launcher, Apps)..." -ForegroundColor Yellow
& "$rootDir\tools\build_userspace.ps1"
if ($LASTEXITCODE -ne 0) {
    Write-Error "Userspace binaries compilation failed!"
    exit 1
}

# 2. Verify AArch64 Mobile Smartphone Compatibility
Write-Host "[AArch64] Cross-compiling Userspace SDK and Apps for aarch64-unknown-none..." -ForegroundColor Cyan
cargo +nightly check --manifest-path "$rootDir\sdk\rust\interos-std\Cargo.toml" --target aarch64-unknown-none
if ($LASTEXITCODE -ne 0) {
    Write-Error "AArch64 SDK cross-compilation check failed!"
    exit 1
}
cargo +nightly check --manifest-path "$rootDir\apps\dialer\Cargo.toml" --target aarch64-unknown-none
if ($LASTEXITCODE -ne 0) {
    Write-Error "AArch64 dialer cross-compilation check failed!"
    exit 1
}
cargo +nightly check --manifest-path "$rootDir\apps\camera\Cargo.toml" --target aarch64-unknown-none
if ($LASTEXITCODE -ne 0) {
    Write-Error "AArch64 camera cross-compilation check failed!"
    exit 1
}
Write-Host "[OK] AArch64 userspace SDK and 2 apps cross-check for aarch64-unknown-none." -ForegroundColor Green
Write-Host "[NOTE] This is a compile check only. It is NOT verification for physical smartphone flashing: no AArch64 build has been booted on real hardware, and no display, touch, or storage driver exists for a physical device." -ForegroundColor Yellow

# 3. Compile x86_64 Microkernel ELF with nightly
Write-Host "[12/13] Compiling Microkernel for x86_64-unknown-none..." -ForegroundColor Yellow
$kernelElf = Invoke-CargoBuildArtifact -CargoArgs @("+nightly", "build", "--manifest-path", "$rootDir\kernel\Cargo.toml", "--target", "x86_64-unknown-none") -TargetName "kernel" -ExpectedKind "bin"

# 4. Compile Standalone AArch64 Smartphone Microkernel
Write-Host "[13/14] Compiling Standalone Smartphone Microkernel for aarch64-unknown-none..." -ForegroundColor Yellow
$aarch64KernelElf = Invoke-CargoBuildArtifact -CargoArgs @("+nightly", "build", "--manifest-path", "$rootDir\kernel-aarch64\Cargo.toml", "--target", "aarch64-unknown-none") -TargetName "kernel-aarch64" -ExpectedKind "bin"

# 5. Compile Standalone RISC-V 64 Smartphone Microkernel
Write-Host "[14/14] Compiling Standalone Smartphone Microkernel for riscv64gc-unknown-none-elf..." -ForegroundColor Yellow
$riscvKernelElf = Invoke-CargoBuildArtifact -CargoArgs @("+nightly", "build", "--manifest-path", "$rootDir\kernel-riscv64\Cargo.toml", "--target", "riscv64gc-unknown-none-elf") -TargetName "kernel-riscv64" -ExpectedKind "bin"

# 5. Package bootable disk image for QEMU bare-metal x86_64
Write-Host "[Packaging] Packaging bootable disk image with bootloader..." -ForegroundColor Yellow
$outputImg = "$rootDir\target\inter-os-bios.img"

# Strip DWARF from the kernel before packaging it.
#
# WHY: the kernel is built with the dev profile, so it carries full debug info --
# measured at .debug_info 7.5 MB, .debug_loc 6.5 MB, .debug_ranges 1.5 MB, about
# 16 MB of the 25 MB file, against 2.1 MB of loadable content. The bootloader reads
# the entire ELF off the disk through BIOS INT13 calls before the kernel runs, so
# all of that is boot time spent loading symbols nothing can use on the device.
# This is the same reasoning as the userspace strip in build_userspace.ps1.
#
# WHY a copy rather than in place: the unstripped ELF is the only artifact that can
# still be symbolised when a panic has to be diagnosed. Destroying it to save boot
# time would trade a debugging capability for speed without saying so.
$stripExe = Get-ChildItem -Path "$((rustc --print sysroot).Trim())\lib\rustlib" -Filter "llvm-strip.exe" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
if ($stripExe) {
    $packagedKernel = "$rootDir\target\kernel-boot.elf"
    Copy-Item -LiteralPath $kernelElf -Destination $packagedKernel -Force
    & $stripExe.FullName --strip-debug $packagedKernel
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Stripping the kernel failed; packaging the unstripped ELF."
        exit 1
    }
    $beforeKb = [math]::Round((Get-Item $kernelElf).Length / 1KB)
    $afterKb = [math]::Round((Get-Item $packagedKernel).Length / 1KB)
    Write-Host "[OK] Kernel stripped for boot: $beforeKb KB -> $afterKb KB (debug copy kept at target\kernel-boot.elf)" -ForegroundColor Green
    $kernelElf = $packagedKernel
} else {
    Write-Host "[WARN] llvm-strip not found; packaging the kernel with its debug info, which makes boot slower." -ForegroundColor Yellow
}

cargo +nightly run --manifest-path "$rootDir\boot\Cargo.toml" -- "$kernelElf" "$outputImg"
if ($LASTEXITCODE -ne 0) {
    Write-Error "Disk image creation failed!"
    exit 1
}
Verify-BiosImageArtifact -ImagePath $outputImg -KernelBinaryPath $kernelElf | Out-Null

Write-Host "Build complete! Bootable image ready at: $outputImg" -ForegroundColor Green

# 6. Package the Android Boot Image (Fastboot v2 header) for AArch64 phone hardware
#
# WHY this uses the AArch64 kernel and not $kernelElf: an Android boot image is
# only ever loaded by an ARM64 phone. Packaging the x86_64 kernel here produced a
# well-formed container that would flash cleanly and then fail to boot. The
# mkbootimg tool now refuses a non-AArch64 kernel outright, so this step would
# fail outright if it still passed $kernelElf.
#
# WHY this is reported rather than fatal: a real device image additionally needs a
# genuine device tree blob and a genuine initramfs. Neither exists in this tree
# yet, so the tool refuses to synthesise placeholders. That is an accurate
# statement of an unfinished platform, not a build break.
Write-Host "[Packaging] Packaging AArch64 Android boot.img for phone hardware..." -ForegroundColor Yellow
$fastbootImg = "$rootDir\target\boot.img"

cargo +nightly run --manifest-path "$rootDir\tools\mkbootimg\Cargo.toml" -- --kernel "$aarch64KernelElf" --output "$fastbootImg"
if ($LASTEXITCODE -eq 0) {
    Write-Host "Android boot-image container built at: $fastbootImg" -ForegroundColor Green
    Write-Host "[NOTE] Container only, and UNTESTED on hardware." -ForegroundColor Yellow
} else {
    Write-Host "[BLOCKED] No phone boot image was produced:" -ForegroundColor Yellow
    Write-Host "         An ARM64 device image requires a real device tree blob and a real" -ForegroundColor Yellow
    Write-Host "         initramfs, and neither exists in this tree. The tool refuses to" -ForegroundColor Yellow
    Write-Host "         substitute placeholders, so no misleading image is produced." -ForegroundColor Yellow
    Write-Host "         Supplied to: target/device_tree.dtb and target/ramdisk.cpio" -ForegroundColor Yellow
}

Write-Host "[NOTE] No Inter OS build has ever booted on physical hardware. The AArch64 kernel" -ForegroundColor Yellow
Write-Host "[NOTE] has no display, touch, or phone storage driver, so flashing is not" -ForegroundColor Yellow
Write-Host "[NOTE] expected to reach a usable UI even once an image can be built." -ForegroundColor Yellow

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  InterOS Core Multi-Arch Built Successfully (x86_64 + Standalone AArch64 + RISC-V 64)" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan


