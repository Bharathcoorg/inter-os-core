$img = (Resolve-Path "$PSScriptRoot\..\target\inter-os-bios.img").Path
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = "C:\Users\bhara\qemu\qemu-system-x86_64.exe"
$psi.Arguments = "-drive format=raw,file=`"$img`" -serial stdio -display none"
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$p = [System.Diagnostics.Process]::Start($psi)
$reader = $p.StandardOutput

$sw = [System.Diagnostics.Stopwatch]::StartNew()
while ($sw.ElapsedMilliseconds -lt 10000) {
    $line = $reader.ReadLine()
    if ($null -ne $line) {
        Write-Host $line
        if ($line -like "*Primordial Kernel Thread registered*") {
            break
        }
    }
}
$p.Kill()


