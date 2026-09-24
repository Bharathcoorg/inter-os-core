# InterOS Core Developer CLI Wrapper (PowerShell)
# Usage: .\interos <command> [args...]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ScriptArgs
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$interosPy = Join-Path $scriptDir "tools\interos.py"

& python $interosPy @ScriptArgs
exit $LASTEXITCODE
