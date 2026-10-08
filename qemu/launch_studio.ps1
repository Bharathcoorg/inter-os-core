$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$img = "$rootDir\target\inter-os-bios.img"
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemu = Resolve-QemuPath "qemu-system-x86_64.exe"
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

$dataPath = if ($env:DATA_DISK_PATH) { $env:DATA_DISK_PATH } else { "$rootDir\target\inter-os-data-master.img" }
if (-not (Test-Path $dataPath)) {
    $qemuImg = Resolve-QemuPath "qemu-img.exe"
    & $qemuImg create -f raw $dataPath 512M | Out-Null
    Write-Host "[SETUP] Created data disk: $dataPath" -ForegroundColor DarkGray
}

# 2. Launch QEMU with VNC display
Write-Host "[QEMU] Launching QEMU with VNC display on port $vncPort..." -ForegroundColor Cyan
$qemuArgs = @(
    "-machine", "q35",
    "-cpu", "qemu64",
    "-m", "1024M",
    "-drive", "format=raw,file=$img,index=0",
    "-drive", "format=raw,file=$dataPath,index=1",
    "-vga", "none",
    "-device", "virtio-vga,xres=720,yres=1440",
    "-device", "qemu-xhci,id=xhci0",
    "-device", "usb-tablet,bus=xhci0.0",
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
if ($qemuProcess -and -not $qemuProcess.HasExited) {
    $qemuProcess | Stop-Process -Force -ErrorAction SilentlyContinue
}
$wsProcess | Stop-Process -Force -ErrorAction SilentlyContinue
$serverProcess | Stop-Process -Force -ErrorAction SilentlyContinue
Write-Host "[SHUTDOWN] All processes stopped." -ForegroundColor Green
