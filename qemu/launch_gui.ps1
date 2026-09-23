$img = "$PSScriptRoot\..\target\inter-os-bios.img"
$qemu = "C:\Users\bhara\qemu\qemu-system-x86_64.exe"

# Stop any existing QEMU processes
Get-Process -Name qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force

Write-Host "Launching QEMU SDL GUI with live interactive window..." -ForegroundColor Cyan
& $qemu -drive format=raw,file=$img -display sdl -serial stdio -monitor tcp:127.0.0.1:44445,server,nowait -name "InterOS Core v0.1"
