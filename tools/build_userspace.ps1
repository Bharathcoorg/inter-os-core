# Build and strip all Ring-3 userspace and app binaries with static relocation
# Architecture: x86_64-unknown-none
# Entry base: 0x00400000

# PowerShell script errors must terminate the build. Native tools (cargo,
# llvm-strip) write progress to stderr, which PowerShell raises as a
# NativeCommandError under Stop, so Invoke-Native drops back to Continue for the
# duration of the call and judges success by exit code instead. Without this a
# failed step falls through and the script reports a successful build.
$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$WorkspaceRoot = Split-Path -Parent $ScriptDir
Set-Location $WorkspaceRoot

# The kernel's ELF loader only accepts ET_EXEC. The `*-unknown-none` targets
# default to position-independent executables, which the linker emits as ET_DYN,
# so -no-pie is required: without it every service is rejected at boot with
# "ELF must be ET_EXEC" and the device comes up with no display or launcher.
$StaticFlags = @(
    "-C", "relocation-model=static",
    "-C", "link-arg=-Ttext=0x00400000",
    "-C", "link-arg=-no-pie",
    # WHY -C opt-level=3: the workspace dev profile only sets panic=abort, so these
    # binaries were built unoptimised. The launcher is a per-pixel software
    # rasteriser, so codegen quality is the difference between a usable frame and an
    # unusable one: measured on a real boot, opt-level 3 alone took a full frame from
    # 121.8s to 99.7s. It does not change behaviour, and the ET_EXEC and fixed-address
    # requirements below are still asserted after every build.
    "-C", "opt-level=3"
)
$UserTargetDir = Join-Path $WorkspaceRoot "target\x86_64-unknown-none\debug"
$ET_EXEC = 2

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)][string]$Exe,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$What
    )
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $Exe @Arguments
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
    if ($code -ne 0) {
        throw "[FAILED] $What exited with $code"
    }
}

function Resolve-StripTool {
    $sysroot = (rustc --print sysroot).Trim()
    $inSysroot = Get-ChildItem -Path "$sysroot\lib\rustlib" -Filter "llvm-strip.exe" -Recurse -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($inSysroot) { return $inSysroot.FullName }
    $onPath = Get-Command "llvm-strip" -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}

# The guest refuses anything that is not a fixed-address executable. Verifying
# here means an unloadable userspace can never be packaged into a boot image.
function Assert-ExecutableElf {
    param([Parameter(Mandatory = $true)][string]$Path)

    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 18) { throw "[FAILED] $Path is too small to be an ELF" }
    if ($bytes[0] -ne 0x7F -or $bytes[1] -ne 0x45 -or $bytes[2] -ne 0x4C -or $bytes[3] -ne 0x46) {
        throw "[FAILED] $Path is not an ELF image"
    }
    $eType = [int]$bytes[16] -bor ([int]$bytes[17] -shl 8)   # little-endian, offset 16
    if ($eType -ne $ET_EXEC) {
        $kind = switch ($eType) { 1 { "ET_REL" } 2 { "ET_EXEC" } 3 { "ET_DYN" } default { "ELF type $eType" } }
        throw "[FAILED] $Path is $kind; the kernel ELF loader only accepts ET_EXEC"
    }
}

# Builds one guest binary, verifies it is loadable, then strips it.
function Invoke-GuestBuild {
    param(
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][string[]]$CargoArgs,
        [Parameter(Mandatory = $true)][string]$Artifact,
        [Parameter(Mandatory = $true)][string]$StripTool
    )

    Write-Host "[*] Building $Label..." -ForegroundColor Yellow
    Invoke-Native -Exe "cargo" -Arguments $CargoArgs -What "cargo ($Label)"

    $targetPath = Join-Path $UserTargetDir $Artifact
    if (-not (Test-Path $targetPath)) {
        throw "[FAILED] $Label reported success but $targetPath was not produced"
    }
    Assert-ExecutableElf -Path $targetPath

    # WHY --strip-debug: bare `llvm-strip` removes the symbol table but KEEPS the
    # DWARF sections. Measured on the authenticator app, .debug_str alone was
    # 302 KB, .debug_info 231 KB and .debug_line 137 KB -- 813 KB of DWARF around
    # 11 KB of actual code, in an 834 KB file. The kernel embeds these binaries, so
    # twelve such apps put roughly 10 MB of debug information into the image, and
    # the signed RAPP artifacts built from them came out at ~878 KB each, far above
    # the 60000-byte transfer limit, so no package could ever be installed.
    # `--strip-debug` keeps the loadable segments the kernel's ELF loader needs.
    Invoke-Native -Exe $StripTool -Arguments @("--strip-debug", $targetPath) -What "llvm-strip ($Artifact)"
    # Stripping must not change the executable type the kernel loader sees.
    Assert-ExecutableElf -Path $targetPath
    Write-Host "  -> ET_EXEC verified and stripped: $Artifact" -ForegroundColor Green
}

$StripExe = Resolve-StripTool
if (-not $StripExe) {
    throw "[FAILED] llvm-strip was not found; guest binaries would remain unstripped"
}

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host " Building Ring-3 Userspace & App Binaries (Static Relocation) " -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan

# 1. Build primordial init
Invoke-GuestBuild -Label "userspace/init" `
    -CargoArgs (@("rustc", "-p", "init", "--target", "x86_64-unknown-none", "--") + $StaticFlags) `
    -Artifact "init" -StripTool $StripExe

# 2. Build the system services.
# sensord must be here: kernel/src/task/images.rs embeds it as ServiceId 11, so
# omitting it leaves a stale artifact in the boot image (or a hard compile error
# on a clean tree).
foreach ($svc in @("displayd", "inputd", "storaged", "netd", "powerd", "modemd", "audiod", "camerad", "sensord")) {
    Invoke-GuestBuild -Label "system service: $svc" `
        -CargoArgs (@("rustc", "-p", "interos-system-service", "--bin", $svc, "--target", "x86_64-unknown-none", "--") + $StaticFlags) `
        -Artifact $svc -StripTool $StripExe
}

# 3. Build UI system apps
foreach ($app in @(
    "compositor", "launcher", "settings", "terminal",
    "dialer", "vault", "camera", "calculator",
    "browser", "messages", "calendar", "music",
    "file_manager", "gallery",
    # WHY these four: kernel/src/task/images.rs embeds them with include_bytes!,
    # and include_bytes! does not trigger a kernel rebuild. An app that is embedded
    # but never built here therefore ships a stale image with no compiler error --
    # and if the file is absent the kernel build fails with a confusing message about
    # a userspace binary rather than about a build script.
    "authenticator", "health", "camera_preview", "hello_world"
)) {
    # WHY `--bin $app`: an app that also exposes a lib target makes the extra rustc
    # flags ambiguous, and cargo then refuses with "extra arguments to `rustc` can
    # only be passed to one target". Naming the binary keeps this working for every
    # app, whether or not it has a library alongside it.
    Invoke-GuestBuild -Label "app: $app" `
        -CargoArgs (@("rustc", "-p", $app, "--bin", $app, "--target", "x86_64-unknown-none", "--") + $StaticFlags) `
        -Artifact $app -StripTool $StripExe
}

# Cross-check the binaries the kernel embeds against what was actually built.
# include_bytes! does not make Cargo rebuild the kernel, so a name that is
# embedded but never built ships as a stale image without any compiler error.
$imagesSource = Join-Path $WorkspaceRoot "kernel\src\task\images.rs"
if (Test-Path $imagesSource) {
    $embedded = [regex]::Matches(
        (Get-Content $imagesSource -Raw),
        'include_bytes!\(concat!\(env!\("CARGO_MANIFEST_DIR"\), "/\.\./target/x86_64-unknown-none/debug/([A-Za-z0-9_]+)"\)'
    ) | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique

    foreach ($name in $embedded) {
        $path = Join-Path $UserTargetDir $name
        if (-not (Test-Path $path)) {
            throw "[FAILED] kernel embeds '$name' but it was not built; the boot image would ship a stale binary"
        }
        Assert-ExecutableElf -Path $path
    }
    Write-Host "[OK] All $($embedded.Count) kernel-embedded user images present and ET_EXEC." -ForegroundColor Green
}

Write-Host "[OK] All Ring-3 userspace binaries built, verified and stripped." -ForegroundColor Green
