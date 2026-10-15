param(
    [string]$AndroidKernel = $env:INTEROS_ANDROID_KERNEL,
    [string]$AndroidRamdisk = $env:INTEROS_ANDROID_RAMDISK,
    [string]$AndroidSystem = $env:INTEROS_ANDROID_SYSTEM,
    [string]$AndroidData = $env:INTEROS_ANDROID_DATA,
    [int]$VncDisplay = 1,
    [int]$MonitorPort = 44446
)

$ErrorActionPreference = "Stop"

foreach ($item in @(
    @{ Name = "INTEROS_ANDROID_KERNEL"; Value = $AndroidKernel },
    @{ Name = "INTEROS_ANDROID_RAMDISK"; Value = $AndroidRamdisk },
    @{ Name = "INTEROS_ANDROID_SYSTEM"; Value = $AndroidSystem },
    @{ Name = "INTEROS_ANDROID_DATA"; Value = $AndroidData }
)) {
    if ([string]::IsNullOrWhiteSpace($item.Value) -or -not (Test-Path -LiteralPath $item.Value)) {
        throw "$($item.Name) must point to a real Android guest image."
    }
}

$qemu = Get-Command qemu-system-x86_64.exe -ErrorAction SilentlyContinue
if (-not $qemu) {
    $qemu = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
}
if (-not $qemu) {
    throw "qemu-system-x86_64 was not found in PATH."
}

$existing = Get-NetTCPConnection -LocalPort $MonitorPort -State Listen -ErrorAction SilentlyContinue
if ($existing) {
    throw "Android compatibility monitor port $MonitorPort is already in use."
}

$arguments = @(
    "-machine", "q35",
    "-cpu", "qemu64",
    "-accel", "tcg",
    "-m", "2048M",
    "-smp", "4",
    "-kernel", $AndroidKernel,
    "-initrd", $AndroidRamdisk,
    "-append", "console=ttyS0",
    "-drive", "if=virtio,format=raw,file=$AndroidSystem,readonly=on",
    "-drive", "if=virtio,format=raw,file=$AndroidData",
    "-device", "virtio-vga,xres=720,yres=1440",
    "-device", "qemu-xhci,id=android-xhci",
    "-device", "usb-tablet,bus=android-xhci.0",
    "-netdev", "user,id=android-net",
    "-device", "virtio-net-pci,netdev=android-net",
    "-serial", "tcp:127.0.0.1:44447,server=on,wait=off",
    "-monitor", "tcp:127.0.0.1:$MonitorPort,server=on,wait=off",
    "-display", "vnc=127.0.0.1:$VncDisplay",
    "-no-reboot"
)

Write-Host "Starting isolated Android guest compatibility VM."
Write-Host "Guest display: 720x1440; VNC: :$VncDisplay; monitor: $MonitorPort"
Write-Host "This VM is the real Android framework/runtime boundary; InterOS remains the host OS."

& $qemu.Source @arguments
