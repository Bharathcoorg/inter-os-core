$log = "$PSScriptRoot\..\target\test_run.log"
if (Test-Path $log) { Remove-Item $log -Force }
$rootDir = (Resolve-Path "$PSScriptRoot\..").Path
$img = "$rootDir\target\inter-os-bios.img"
. (Join-Path $rootDir "tools\qemu_env.ps1")
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = Resolve-QemuPath "qemu-system-x86_64.exe"
$psi.Arguments = "-machine q35 -cpu qemu64 -m 1024M -drive format=raw,file=`"$img`" -serial stdio -display none -no-reboot"
$psi.RedirectStandardOutput = $true
$psi.UseShellExecute = $false
$p = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Seconds 4
if (-not $p.HasExited) {
    $p.Kill()
}
$output = $p.StandardOutput.ReadToEnd()
$output | Out-File -FilePath $log -Encoding utf8
$output.Split("`n") | Select-String -Pattern "\[TEST\]" | Select-Object -Last 20

