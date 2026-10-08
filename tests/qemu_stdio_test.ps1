$rootDir = (Resolve-Path "$PSScriptRoot\..").Path
$img = (Resolve-Path "$rootDir\target\inter-os-bios.img").Path
. (Join-Path $rootDir "tools\qemu_env.ps1")
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = Resolve-QemuPath "qemu-system-x86_64.exe"
$psi.Arguments = "-machine q35 -cpu qemu64 -m 1024M -drive format=raw,file=`"$img`" -serial stdio -display none"
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


