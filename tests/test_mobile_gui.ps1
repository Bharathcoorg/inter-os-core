# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$img = Join-Path $rootDir "target\inter-os-bios.img"
. (Join-Path $rootDir "tools\qemu_env.ps1")
$qemu = Resolve-QemuPath "qemu-system-x86_64.exe"
$ppmPath = Join-Path $rootDir "target\mobile_screen.ppm"
$pngPath = Join-Path $rootDir "target\mobile_screen.png"
$artifactPath = if ($env:ARTIFACT_PATH) { $env:ARTIFACT_PATH } else { Join-Path $rootDir "target\inter_os_mobile_screen.png" }

if (Test-Path $ppmPath) { Remove-Item $ppmPath -Force }
if (Test-Path $pngPath) { Remove-Item $pngPath -Force }

Write-Host "Starting QEMU in headless verification mode to capture mobile GUI framebuffer..." -ForegroundColor Cyan
$p = Start-Process -FilePath $qemu -ArgumentList "-machine q35 -cpu qemu64 -m 1024M -drive format=raw,file=`"$img`" -display none -monitor tcp:127.0.0.1:44446,server,nowait -serial stdio -no-reboot" -PassThru
Start-Sleep -Seconds 10

Write-Host "Connecting to QEMU monitor on port 44446..." -ForegroundColor Yellow
$tcp = New-Object System.Net.Sockets.TcpClient
$tcp.Connect("127.0.0.1", 44446)
$ns = $tcp.GetStream()
$sw = New-Object System.IO.StreamWriter($ns)
$sw.WriteLine("screendump target/mobile_screen.ppm")
$sw.Flush()
Start-Sleep -Milliseconds 1200
$tcp.Close()

Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue

if (Test-Path $ppmPath) {
    Write-Host "PPM file captured: $( (Get-Item $ppmPath).Length ) bytes" -ForegroundColor Green
    Add-Type -AssemblyName System.Drawing
    $bytes = [System.IO.File]::ReadAllBytes($ppmPath)
    
    $headerStr = [System.Text.Encoding]::ASCII.GetString($bytes[0..100])
    $match = [regex]::Match($headerStr, "P6\s+(\d+)\s+(\d+)\s+(\d+)\s")
    if ($match.Success) {
        $w = [int]$match.Groups[1].Value
        $h = [int]$match.Groups[2].Value
        $headerLen = $match.Length
        
        $bmp = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        $bmpData = $bmp.LockBits((New-Object System.Drawing.Rectangle(0, 0, $w, $h)), [System.Drawing.Imaging.ImageLockMode]::WriteOnly, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        
        $ptr = $bmpData.Scan0
        $rgbBytes = New-Object byte[] ($w * $h * 3)
        $srcIdx = $headerLen
        $dstIdx = 0
        for ($i = 0; $i -lt ($w * $h); $i++) {
            $r = $bytes[$srcIdx]
            $g = $bytes[$srcIdx + 1]
            $b = $bytes[$srcIdx + 2]
            $rgbBytes[$dstIdx] = $b
            $rgbBytes[$dstIdx + 1] = $g
            $rgbBytes[$dstIdx + 2] = $r
            $srcIdx += 3
            $dstIdx += 3
        }
        [System.Runtime.InteropServices.Marshal]::Copy($rgbBytes, 0, $ptr, $rgbBytes.Length)
        $bmp.UnlockBits($bmpData)
        $bmp.Save($pngPath, [System.Drawing.Imaging.ImageFormat]::Png)
        $bmp.Dispose()
        Write-Host "PNG screenshot generated: $pngPath" -ForegroundColor Green

        Copy-Item $pngPath $artifactPath -Force
        Write-Host "Copied to artifact: $artifactPath" -ForegroundColor Green
    }
} else {
    Write-Error "Failed to capture PPM screenshot from QEMU monitor."
}
