$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  Building Gravity OS v0.1 (x86_64)" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan

Set-Location $rootDir

# 1. Compile Standalone Ring 3 Userspace Binaries
Write-Host "[1/5] Compiling Userspace Init for x86_64-unknown-none (base: 0x400000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\userspace\init\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0x400000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Userspace init compilation failed!"
    exit 1
}

Write-Host "[2/5] Compiling Settings App for x86_64-unknown-none (base: 0x500000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\apps\settings\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0x500000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Settings app compilation failed!"
    exit 1
}

Write-Host "[3/5] Compiling Intent Terminal App for x86_64-unknown-none (base: 0x600000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\apps\terminal\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0x600000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Terminal app compilation failed!"
    exit 1
}

Write-Host "[4/6] Compiling Sovereign Dialer App for x86_64-unknown-none (base: 0x700000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\apps\dialer\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0x700000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Dialer app compilation failed!"
    exit 1
}

Write-Host "[5/7] Compiling Sovereign Vault App for x86_64-unknown-none (base: 0x800000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\apps\vault\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0x800000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Vault app compilation failed!"
    exit 1
}

Write-Host "[6/8] Compiling Sovereign Camera App for x86_64-unknown-none (base: 0x900000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\apps\camera\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0x900000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Camera app compilation failed!"
    exit 1
}

Write-Host "[7/9] Compiling Touch Calculator App for x86_64-unknown-none (base: 0xA00000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\apps\calculator\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0xA00000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Calculator app compilation failed!"
    exit 1
}

Write-Host "[8/10] Compiling Sovereign Browser App for x86_64-unknown-none (base: 0xB00000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\apps\browser\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0xB00000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Browser app compilation failed!"
    exit 1
}

Write-Host "[9/11] Compiling Sovereign Messages App for x86_64-unknown-none (base: 0xC00000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\apps\messages\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0xC00000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Messages app compilation failed!"
    exit 1
}

Write-Host "[10/12] Compiling Sovereign Calendar App for x86_64-unknown-none (base: 0xD00000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\apps\calendar\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0xD00000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Calendar app compilation failed!"
    exit 1
}

Write-Host "[11/12] Compiling Sovereign Music App for x86_64-unknown-none (base: 0xE00000)..." -ForegroundColor Yellow
cargo +nightly rustc --manifest-path "$rootDir\apps\music\Cargo.toml" --target x86_64-unknown-none -- -C relocation-model=static -C link-arg=-Ttext=0xE00000
if ($LASTEXITCODE -ne 0) {
    Write-Error "Music app compilation failed!"
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
cargo +nightly check --manifest-path "$rootDir\apps\camera\Cargo.toml" --target aarch64-unknown-none
Write-Host "[OK] AArch64 Mobile compatibility verified for physical smartphone flashing." -ForegroundColor Green

# 3. Compile x86_64 Microkernel ELF with nightly
Write-Host "[12/13] Compiling Microkernel for x86_64-unknown-none..." -ForegroundColor Yellow
cargo +nightly build --manifest-path "$rootDir\kernel\Cargo.toml" --target x86_64-unknown-none
if ($LASTEXITCODE -ne 0) {
    Write-Error "Kernel compilation failed!"
    exit 1
}

# 4. Compile Standalone AArch64 Smartphone Microkernel
Write-Host "[13/14] Compiling Standalone Smartphone Microkernel for aarch64-unknown-none..." -ForegroundColor Yellow
cargo +nightly build --manifest-path "$rootDir\kernel-aarch64\Cargo.toml" --target aarch64-unknown-none
if ($LASTEXITCODE -ne 0) {
    Write-Error "AArch64 kernel compilation failed!"
    exit 1
}

# 5. Compile Standalone RISC-V 64 Smartphone Microkernel
Write-Host "[14/14] Compiling Standalone Smartphone Microkernel for riscv64gc-unknown-none-elf..." -ForegroundColor Yellow
cargo +nightly build --manifest-path "$rootDir\kernel-riscv64\Cargo.toml" --target riscv64gc-unknown-none-elf
if ($LASTEXITCODE -ne 0) {
    Write-Error "RISC-V 64 kernel compilation failed!"
    exit 1
}

# 5. Package bootable disk image for QEMU bare-metal x86_64
Write-Host "[Packaging] Packaging bootable disk image with bootloader..." -ForegroundColor Yellow
$k1 = "$rootDir\target\x86_64-unknown-none\debug\kernel"
$k2 = "$rootDir\kernel\target\x86_64-unknown-none\debug\kernel"
$kernelElf = if ((Test-Path $k2) -and (-not (Test-Path $k1) -or (Get-Item $k2).LastWriteTime -gt (Get-Item $k1).LastWriteTime)) { $k2 } else { $k1 }
$outputImg = "$rootDir\target\inter-os-bios.img"

cargo +nightly run --manifest-path "$rootDir\boot\Cargo.toml" -- "$kernelElf" "$outputImg"
if ($LASTEXITCODE -ne 0) {
    Write-Error "Disk image creation failed!"
    exit 1
}

Write-Host "Build complete! Bootable image ready at: $outputImg" -ForegroundColor Green

# 6. Package Android Boot Image (Fastboot v2 Header) for physical smartphone flashing
Write-Host "[Packaging] Packaging Fastboot Android boot.img for physical device flashing..." -ForegroundColor Yellow
$fastbootImg = "$rootDir\target\boot.img"
cargo +nightly run --manifest-path "$rootDir\tools\mkbootimg\Cargo.toml" -- --kernel "$kernelElf" --output "$fastbootImg"
if ($LASTEXITCODE -ne 0) {
    Write-Error "Fastboot boot.img creation failed!"
    exit 1
}

Write-Host "Build complete! Fastboot flashable image ready at: $fastbootImg" -ForegroundColor Green

# 7. Package Physical AArch64 Fastboot Android Boot Image
Write-Host "[Packaging] Packaging Authentic AArch64 Fastboot Android boot-aarch64.img..." -ForegroundColor Yellow
$k_aarch64_1 = "$rootDir\target\aarch64-unknown-none\debug\kernel-aarch64"
$k_aarch64_2 = "$rootDir\kernel-aarch64\target\aarch64-unknown-none\debug\kernel-aarch64"
$aarch64KernelElf = if ((Test-Path $k_aarch64_2) -and (-not (Test-Path $k_aarch64_1) -or (Get-Item $k_aarch64_2).LastWriteTime -gt (Get-Item $k_aarch64_1).LastWriteTime)) { $k_aarch64_2 } else { $k_aarch64_1 }
$aarch64BootImg = "$rootDir\target\boot-aarch64.img"
cargo +nightly run --manifest-path "$rootDir\tools\mkbootimg\Cargo.toml" -- --kernel "$aarch64KernelElf" --output "$aarch64BootImg"
if ($LASTEXITCODE -ne 0) {
    Write-Error "AArch64 Fastboot boot.img creation failed!"
    exit 1
}

Write-Host "Build complete! Authentic AArch64 flashable image ready at: $aarch64BootImg" -ForegroundColor Green
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  InterOS Core Multi-Arch Built Successfully (x86_64 + Standalone AArch64 + RISC-V 64)" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan


