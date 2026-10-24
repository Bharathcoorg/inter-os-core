@echo off
rem Desktop QEMU launcher for the InterOS native 720x1440 scanout.
rem
rem WHY the arguments mirror emulator/server.py's x86_64 profile rather than
rem being invented here: the Studio launcher already documents (in that file)
rem why the standard VGA device is used instead of virtio-vga, why the data disk
rem is mandatory, and why the accelerator chain ends in TCG. Two launchers for
rem one guest must not disagree about any of it.
rem
rem WHY the text below is plain ASCII: cmd.exe reads a .bat in the console
rem code page, so a UTF-8 em dash prints as mojibake. The Python side can carry
rem UTF-8 because it declares its own encoding; a batch file cannot.
setlocal
title InterOS Core Native Portrait (720x1440)
cd /d "%~dp0"

echo ========================================================
echo   InterOS Core - Native Portrait Scanout
echo   Display: GTK 720x1440 Portrait (auto-fit to monitor)
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
    echo         Build it first: python tools\interos.py build
    pause
    exit /b 1
)

rem WHY a second disk is not optional: the kernel treats the Primary Master as
rem the read-only boot image and the Primary Slave as persistent storage. Handed
rem only one disk it formats the boot image itself, overwriting the bootloader
rem stages the kernel is embedded in, and the next boot runs a corrupted kernel.
rem 512MB is the size emulator/server.py's ensure_data_disk() uses platform-wide.
if not exist "%DATA_IMG%" (
    echo [INFO] Data disk not found; creating the 512MB second disk...
    powershell -NoProfile -Command ^
      "$f='%DATA_IMG%'; New-Item -ItemType File -Path $f -Force | Out-Null; $s=[IO.File]::Open($f,'OpenOrCreate','Write','None'); $s.SetLength(536870912); $s.Close()"
    if not exist "%DATA_IMG%" (
        echo [ERROR] Could not create %DATA_IMG%
        pause
        exit /b 1
    )
)

rem WHY the accelerator is a list and not a single -accel: server.py keeps plain
rem TCG as the last candidate so a WHPX that cannot start this machine falls
rem back instead of failing the launch. A hard-coded -accel whpx made this
rem launcher the one place in the tree that could not start at all on a host
rem without WHPX. QEMU tries each -accel in order and reports which one it used.
echo [INFO] Launching QEMU Native Portrait Window...
"%QEMU_BIN%" -name "InterOS Core - Native Portrait (720x1440)" -m 1024M -smp 1 -machine q35 -cpu qemu64 -display gtk,zoom-to-fit=on -vga none -device VGA,edid=on,xres=720,yres=1440,vgamem_mb=32 -drive "format=raw,file=%BOOT_IMG%,index=0" -drive "format=raw,file=%DATA_IMG%,index=1" -netdev user,id=net0 -device e1000,netdev=net0,mac=52:54:00:12:34:56 -monitor tcp:127.0.0.1:44445,server,nowait -serial tcp:127.0.0.1:4301,server,nowait -vnc 127.0.0.1:1 -accel whpx,kernel-irqchip=off -accel tcg

pause