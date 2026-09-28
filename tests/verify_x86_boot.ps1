$ErrorActionPreference = "Stop"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = Split-Path -Parent $scriptDir
$pyScript = Join-Path $scriptDir "verify_x86_boot.py"

& python $pyScript
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
exit 0
