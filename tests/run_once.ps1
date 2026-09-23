Stop-Process -Name "qemu-system-x86_64" -Force -ErrorAction SilentlyContinue
$log = "$PSScriptRoot\..\target\test_run.log"
if (Test-Path $log) { Remove-Item $log -Force }
$img = "$PSScriptRoot\..\target\inter-os-bios.img"
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = "C:\Users\bhara\qemu\qemu-system-x86_64.exe"
$psi.Arguments = "-drive format=raw,file=`"$img`" -serial stdio -display none -no-reboot"
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

