$ErrorActionPreference = "Continue"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$img = Join-Path $rootDir "target\inter-os-bios.img"
$qemu = "C:\Users\bhara\qemu\qemu-system-x86_64.exe"
$logPath = Join-Path $rootDir "target\x86_boot.log"
$ppmPath = Join-Path $rootDir "target\mobile_screen2.ppm"
$pngPath = Join-Path $rootDir "target\mobile_screen2.png"

Get-Process -Name "qemu-system-x86_64" -ErrorAction SilentlyContinue | Stop-Process -Force
if (Test-Path $logPath) { Remove-Item $logPath -Force }
if (Test-Path $ppmPath) { Remove-Item $ppmPath -Force }
if (Test-Path $pngPath) { Remove-Item $pngPath -Force }

Write-Host "Launching QEMU x86_64 with 1024M RAM..." -ForegroundColor Cyan
$p = Start-Process -FilePath $qemu -ArgumentList "-m 1024M -drive format=raw,file=`"$img`" -display none -monitor tcp:127.0.0.1:44447,server,nowait -serial file:`"$logPath`" -no-reboot" -PassThru

Start-Sleep -Seconds 60

try {
    $tcp = New-Object System.Net.Sockets.TcpClient
    $tcp.Connect("127.0.0.1", 44447)
    $ns = $tcp.GetStream()
    $sw = New-Object System.IO.StreamWriter($ns)
    $sw.WriteLine("screendump target/mobile_screen2.ppm")
    $sw.Flush()
    Start-Sleep -Milliseconds 1200
    $tcp.Close()
} catch {
    Write-Warning "Monitor screendump failed: $_"
}

if (-not $p.HasExited) {
    $p.Kill()
}

Write-Host "Checking serial log..." -ForegroundColor Yellow
if (Test-Path $logPath) {
    $lines = Get-Content $logPath
    Write-Host "Serial log lines: $($lines.Count)"
    $lines | Select-Object -Last 30
} else {
    Write-Warning "No serial log found."
}

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
        Write-Host "PNG saved to $pngPath" -ForegroundColor Green
    }
}
