$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$img = "$rootDir\target\inter-os-bios.img"
$qemu = "C:\Users\bhara\qemu\qemu-system-x86_64.exe"
$emulatorDir = "$rootDir\emulator"

# Ports
$vncPort = 5900
$wsPort = 6080
$httpPort = 4300

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "  INTER OS — Live Device Studio Launcher" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host ""

# 1. Check that the kernel image exists
if (-not (Test-Path $img)) {
    Write-Host "[BUILD] Kernel image not found. Building..." -ForegroundColor Yellow
    & "$scriptDir\build.ps1"
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Kernel build failed!"
        exit 1
    }
}

# 2. Kill any lingering QEMU / websockify / python server processes
Write-Host "[CLEANUP] Stopping any existing processes..." -ForegroundColor Yellow
Get-Process -Name qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force
Get-Process -Name websockify -ErrorAction SilentlyContinue | Stop-Process -Force

# 3. Launch QEMU with VNC display (no local SDL window — browser-only)
Write-Host "[QEMU] Launching QEMU with VNC display on port $vncPort..." -ForegroundColor Cyan
$qemuArgs = @(
    "-drive", "format=raw,file=$img",
    "-display", "vnc=:0",
    "-serial", "tcp:127.0.0.1:4201,server,nowait",
    "-monitor", "tcp:127.0.0.1:44445,server,nowait",
    "-name", "Inter OS v0.1"
)
$qemuProcess = Start-Process -FilePath $qemu -ArgumentList $qemuArgs -PassThru -NoNewWindow
Write-Host "[QEMU] Started (PID: $($qemuProcess.Id))" -ForegroundColor Green

# 4. Wait a moment for QEMU VNC to be ready
Start-Sleep -Milliseconds 800

# 5. Launch websockify (VNC TCP → WebSocket bridge)
Write-Host "[WEBSOCKIFY] Bridging VNC:$vncPort -> WebSocket:$wsPort..." -ForegroundColor Cyan
$wsProcess = Start-Process -FilePath "python" -ArgumentList "-m", "websockify", "$wsPort", "localhost:$vncPort" -PassThru -NoNewWindow
Write-Host "[WEBSOCKIFY] Started (PID: $($wsProcess.Id))" -ForegroundColor Green

# 6. Launch the studio HTTP server
Write-Host "[STUDIO] Starting browser studio on http://localhost:$httpPort ..." -ForegroundColor Cyan
$serverProcess = Start-Process -FilePath "python" -ArgumentList "$emulatorDir\server.py" -PassThru -NoNewWindow
Write-Host "[STUDIO] Started (PID: $($serverProcess.Id))" -ForegroundColor Green

# 7. Open the browser
Start-Sleep -Milliseconds 500
$url = "http://localhost:$httpPort"
Write-Host ""
Write-Host "============================================================" -ForegroundColor Green
Write-Host "  INTER OS Studio is LIVE" -ForegroundColor Green
Write-Host "  Browser:    $url" -ForegroundColor White
Write-Host "  QEMU VNC:   localhost:$vncPort" -ForegroundColor White
Write-Host "  WebSocket:  ws://localhost:$wsPort" -ForegroundColor White
Write-Host "  Serial:     tcp://localhost:4201" -ForegroundColor White
Write-Host "============================================================" -ForegroundColor Green
Write-Host ""
Write-Host "Press Ctrl+C to stop all processes." -ForegroundColor Yellow

try {
    Start-Process $url
} catch {}

# 8. Wait for QEMU to exit, then clean up
try {
    $qemuProcess.WaitForExit()
} catch {}

Write-Host "[SHUTDOWN] Cleaning up..." -ForegroundColor Yellow
$wsProcess | Stop-Process -Force -ErrorAction SilentlyContinue
$serverProcess | Stop-Process -Force -ErrorAction SilentlyContinue
Write-Host "[SHUTDOWN] All processes stopped." -ForegroundColor Green
