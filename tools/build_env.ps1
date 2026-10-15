# InterOS Build Environment — Structured Artifact Verification & Build Helpers
# Resolves and validates compiler output via Cargo structured messages.

function Invoke-CargoBuildArtifact {
    param(
        [Parameter(Mandatory=$true)]
        [string[]]$CargoArgs,
        [Parameter(Mandatory=$true)]
        [string]$TargetName,
        [string]$ExpectedKind = "bin",
        [string]$WorkingDir = ""
    )

    $savedDir = $null
    if ($WorkingDir -and (Test-Path $WorkingDir)) {
        $savedDir = Get-Location
        Set-Location $WorkingDir
    }

    try {
        $fullArgs = @()
        $hasMsgFormat = $false
        foreach ($arg in $CargoArgs) {
            if ($arg -like "--message-format*") {
                $hasMsgFormat = $true
            }
            $fullArgs += $arg
        }
        if (-not $hasMsgFormat) {
            $fullArgs += "--message-format=json"
        }

        $oldEap = $ErrorActionPreference
        try {
            $ErrorActionPreference = "SilentlyContinue"
            $output = & cargo @fullArgs 2>&1
            $cargoExitCode = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $oldEap
        }

        $artifactPath = $null
        $isFresh = $false
        $buildSuccess = $false
        $compilerMessages = @()

        foreach ($line in $output) {
            $lineStr = if ($line -is [System.Management.Automation.ErrorRecord]) {
                $line.ToString()
            } else {
                [string]$line
            }
            $trimmed = $lineStr.Trim()

            if ($trimmed.StartsWith("{") -and $trimmed.EndsWith("}")) {
                try {
                    $json = $trimmed | ConvertFrom-Json
                    if ($json.reason -eq "compiler-artifact" -and $json.target.name -eq $TargetName) {
                        $kinds = @($json.target.kind)
                        if ($kinds -contains $ExpectedKind) {
                            if ($json.executable) {
                                $artifactPath = [string]$json.executable
                            } elseif ($json.filenames -and $json.filenames.Count -gt 0) {
                                $artifactPath = [string]$json.filenames[0]
                            }
                            $isFresh = [bool]$json.fresh
                        }
                    } elseif ($json.reason -eq "build-finished") {
                        $buildSuccess = [bool]$json.success
                    } elseif ($json.reason -eq "compiler-message" -and $json.message -and $json.message.rendered) {
                        $compilerMessages += [string]$json.message.rendered
                    }
                } catch {
                    if ($lineStr) {
                        Write-Host $lineStr
                    }
                }
            } else {
                if ($lineStr) {
                    Write-Host $lineStr
                }
            }
        }

        if ($cargoExitCode -ne 0 -or (-not $buildSuccess)) {
            if ($compilerMessages.Count -gt 0) {
                foreach ($msg in $compilerMessages) {
                    Write-Host $msg -ForegroundColor Red
                }
            }
            throw "Cargo build failed for '$TargetName' with exit code $cargoExitCode"
        }

        if (-not $artifactPath) {
            throw "Cargo build succeeded but no compiler-artifact was reported for target '$TargetName' ($ExpectedKind)"
        }

        # Normalize path separators for current platform
        $artifactPath = [System.IO.Path]::GetFullPath($artifactPath)

        if (-not (Test-Path -LiteralPath $artifactPath)) {
            throw "Verified build artifact path does not exist on disk: $artifactPath"
        }

        $item = Get-Item -LiteralPath $artifactPath
        if ($item.Length -eq 0) {
            throw "Verified build artifact is empty (0 bytes): $artifactPath"
        }

        # Verify ELF header for executable binary
        if ($ExpectedKind -eq "bin") {
            $fs = [System.IO.File]::OpenRead($artifactPath)
            try {
                $buf = New-Object byte[] 4
                $n = $fs.Read($buf, 0, 4)
                if ($n -lt 4 -or $buf[0] -ne 0x7F -or $buf[1] -ne 0x45 -or $buf[2] -ne 0x4C -or $buf[3] -ne 0x46) {
                    throw "Artifact at $artifactPath is not a valid ELF binary (invalid magic header)"
                }
            } finally {
                $fs.Close()
            }
        }

        $statusStr = if ($isFresh) { "cached/up-to-date" } else { "recompiled" }
        Write-Host "[OK] $TargetName binary verified ($statusStr): $artifactPath ($($item.Length) bytes)" -ForegroundColor Green

        return $artifactPath
    } finally {
        if ($savedDir) {
            Set-Location $savedDir
        }
    }
}

function Verify-BiosImageArtifact {
    param(
        [Parameter(Mandatory=$true)]
        [string]$ImagePath
    )

    $resolved = [System.IO.Path]::GetFullPath($ImagePath)
    if (-not (Test-Path -LiteralPath $resolved)) {
        throw "Missing build artifact: BIOS image not found at $resolved"
    }

    $item = Get-Item -LiteralPath $resolved
    if ($item.Length -lt 512) {
        throw "Build artifact verification failed: BIOS image is smaller than one sector ($($item.Length) bytes) at $resolved"
    }

    $fs = [System.IO.File]::OpenRead($resolved)
    try {
        $sector = New-Object byte[] 512
        $readBytes = $fs.Read($sector, 0, 512)
        if ($readBytes -lt 512 -or $sector[510] -ne 0x55 -or $sector[511] -ne 0xAA) {
            throw "Build artifact verification failed: BIOS image at $resolved lacks valid MBR boot signature (0x55, 0xAA)"
        }
    } finally {
        $fs.Close()
    }

    Write-Host "[OK] BIOS image verified: $resolved ($($item.Length) bytes)" -ForegroundColor Green
    return $resolved
}
