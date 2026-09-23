@echo off
title InterOS Core v0.1 - Mobile Microkernel
echo ========================================================
echo   Launching InterOS Core v0.1 in QEMU Graphical Window...
echo ========================================================
echo.
echo Stopping any lingering QEMU instances...
taskkill /F /IM qemu-system-x86_64.exe 2>nul
echo.
echo Booting InterOS Core (1280x720 Graphical Framebuffer)...
"C:\Users\bhara\qemu\qemu-system-x86_64.exe" -drive format=raw,file="%~dp0target\inter-os-bios.img" -serial stdio -name "InterOS Core v0.1"
pause
