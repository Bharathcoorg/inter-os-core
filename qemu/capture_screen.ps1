$ppmPath = "target\screen.ppm"
$pngPath = "target\screen.png"

$tcp = New-Object System.Net.Sockets.TcpClient
$tcp.Connect("127.0.0.1", 44445)
$ns = $tcp.GetStream()
$sw = New-Object System.IO.StreamWriter($ns)
$sw.WriteLine("screendump target/screen.ppm")
$sw.Flush()
Start-Sleep -Milliseconds 800
$tcp.Close()

if (Test-Path $ppmPath) {
    Write-Host "PPM file captured: $( (Get-Item $ppmPath).Length ) bytes"
    # Convert PPM P6 to PNG
    Add-Type -AssemblyName System.Drawing
    $bytes = [System.IO.File]::ReadAllBytes($ppmPath)
    
    # Parse P6 header
    $headerStr = [System.Text.Encoding]::ASCII.GetString($bytes[0..100])
    $match = [regex]::Match($headerStr, "P6\s+(\d+)\s+(\d+)\s+(\d+)\s")
    if ($match.Success) {
        $w = [int]$match.Groups[1].Value
        $h = [int]$match.Groups[2].Value
        $headerLen = $match.Length
        
        $bmp = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        $bmpData = $bmp.LockBits((New-Object System.Drawing.Rectangle(0, 0, $w, $h)), [System.Drawing.Imaging.ImageLockMode]::WriteOnly, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        
        # P6 is RGB, GDI+ 24bppRgb is BGR
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
    }
} else {
    Write-Error "Failed to capture PPM screenshot from QEMU monitor."
}
