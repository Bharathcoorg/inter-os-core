@echo off
setlocal enabledelayedexpansion
title InterOS Core Full HD (1080x2160)
cd /d "%~dp0"

echo ========================================================
echo   InterOS Core — Native Full HD Desktop Scanout
echo   Display: GTK 1080x2160 Portrait (Auto-fit to monitor)
echo   Studio Bridge: VNC 5901 + Serial 4301 + Monitor 44445
echo ========================================================
echo.

set "QEMU_BIN=qemu-system-x86_64.exe"
if exist "%USERPROFILE%\qemu\qemu-system-x86_64.exe" set "QEMU_BIN=%USERPROFILE%\qemu\qemu-system-x86_64.exe"
if exist "C:\Program Files\qemu\qemu-system-x86_64.exe" set "QEMU_BIN=C:\Program Files\qemu\qemu-system-x86_64.exe"

set "BOOT_IMG=%~dp0target\inter-os-bios.img"
set "DATA_IMG=%~dp0target\inter-os-data-master.img"

if not exist "%BOOT_IMG%" (
    echo [ERROR] Boot image not found at %BOOT_IMG%
    echo Please build first: python tools\interos.py build
    pause
    exit /b 1
)

if not exist "%DATA_IMG%" (
    echo [INFO] Data disk not found; creating 32MB storage disk...
    fsutil file createnew "%DATA_IMG%" 33554432 >nul 2>&1
)

echo [INFO] Launching QEMU Native Full HD Desktop Window...
"%QEMU_BIN%" -name "InterOS Core — Native Full HD (1080x2160)" -m 1024M -smp 1 -machine q35 -cpu qemu64 -display gtk,zoom-to-fit=on -vga none -device VGA,edid=on,xres=1080,yres=2160,vgamem_mb=32 -drive "format=raw,file=%BOOT_IMG%,index=0" -drive "format=raw,file=%DATA_IMG%,index=1" -netdev user,id=net0 -device e1000,netdev=net0,mac=52:54:00:12:34:56 -monitor tcp:127.0.0.1:44445,server,nowait -serial tcp:127.0.0.1:4301,server,nowait -vnc 127.0.0.1:1 -accel whpx,kernel-irqchip=off

pause
