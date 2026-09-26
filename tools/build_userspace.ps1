# Build and strip all Ring-3 userspace and app binaries with static relocation
# Architecture: x86_64-unknown-none
# Entry base: 0x00400000

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$WorkspaceRoot = Split-Path -Parent $ScriptDir
Set-Location $WorkspaceRoot

$SysRoot = (rustc --print sysroot).Trim()
$StripExe = (Get-ChildItem -Path "$SysRoot\lib\rustlib" -Filter "llvm-strip.exe" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1).FullName
if (-not $StripExe) {
    $found = Get-Command "llvm-strip" -ErrorAction SilentlyContinue
    if ($found) { $StripExe = $found.Source }
}

$StaticFlags = @("-C", "relocation-model=static", "-C", "link-arg=-Ttext=0x00400000")

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host " Building Ring-3 Userspace & App Binaries (Static Relocation) " -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan

# 1. Build primordial init
Write-Host "`n[*] Building userspace/init..." -ForegroundColor Yellow
cargo rustc -p init --target x86_64-unknown-none -- @StaticFlags
if (Test-Path "$WorkspaceRoot\target\x86_64-unknown-none\debug\init") {
    & $StripExe "$WorkspaceRoot\target\x86_64-unknown-none\debug\init"
    Write-Host "  -> Stripped init" -ForegroundColor Green
}

# 2. Build 8 system services
$services = @("displayd", "inputd", "storaged", "netd", "powerd", "modemd", "audiod", "camerad")
foreach ($svc in $services) {
    Write-Host "`n[*] Building system service: $svc..." -ForegroundColor Yellow
    cargo rustc -p interos-system-service --bin $svc --target x86_64-unknown-none -- @StaticFlags
    $targetPath = "$WorkspaceRoot\target\x86_64-unknown-none\debug\$svc"
    if (Test-Path $targetPath) {
        & $StripExe $targetPath
        Write-Host "  -> Stripped $svc" -ForegroundColor Green
    }
}

# 3. Build UI system apps
$apps = @(
    "compositor", "launcher", "settings", "terminal",
    "dialer", "vault", "camera", "calculator",
    "browser", "messages", "calendar", "music"
)

foreach ($app in $apps) {
    Write-Host "`n[*] Building app: $app..." -ForegroundColor Yellow
    cargo rustc -p $app --target x86_64-unknown-none -- @StaticFlags
    $targetPath = "$WorkspaceRoot\target\x86_64-unknown-none\debug\$app"
    if (Test-Path $targetPath) {
        & $StripExe $targetPath
        Write-Host "  -> Stripped $app" -ForegroundColor Green
    }
}

Write-Host "`n[OK] All Ring-3 userspace binaries built and stripped successfully!" -ForegroundColor Green
