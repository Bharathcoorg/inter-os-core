@echo off
title InterOS Core v0.1 - Mobile Microkernel
echo ========================================================
echo   Launching InterOS Core v0.1 in QEMU Graphical Window...
echo ========================================================
echo.
echo Stopping any lingering QEMU instances...
taskkill /F /IM qemu-system-x86_64.exe 2>nul
echo.
set "QEMU_BIN=qemu-system-x86_64.exe"
if exist "%USERPROFILE%\qemu\qemu-system-x86_64.exe" set "QEMU_BIN=%USERPROFILE%\qemu\qemu-system-x86_64.exe"
if exist "C:\Program Files\qemu\qemu-system-x86_64.exe" set "QEMU_BIN=C:\Program Files\qemu\qemu-system-x86_64.exe"
"%QEMU_BIN%" -drive format=raw,file="%~dp0target\inter-os-bios.img" -serial stdio -name "InterOS Core v0.1"
pause
