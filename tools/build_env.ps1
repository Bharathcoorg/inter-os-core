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

        if ($ExpectedKind -eq "bin") {
            $expectedArch = ""
            foreach ($arg in $CargoArgs) {
                if ($arg -like "*x86_64*") { $expectedArch = "x86_64" }
                elseif ($arg -like "*aarch64*") { $expectedArch = "aarch64" }
                elseif ($arg -like "*riscv64*") { $expectedArch = "riscv64" }
            }
            Verify-KernelBinaryArtifact -BinaryPath $artifactPath -ExpectedArch $expectedArch | Out-Null
        } else {
            if (-not (Test-Path -LiteralPath $artifactPath)) {
                throw "Verified build artifact path does not exist on disk: $artifactPath"
            }
            $item = Get-Item -LiteralPath $artifactPath
            if ($item.Length -eq 0) {
                throw "Verified build artifact is empty (0 bytes): $artifactPath"
            }
        }

        $item = Get-Item -LiteralPath $artifactPath
        $statusStr = if ($isFresh) { "cached/up-to-date" } else { "recompiled" }
        Write-Host "[OK] $TargetName binary verified ($statusStr): $artifactPath ($($item.Length) bytes)" -ForegroundColor Green

        return $artifactPath
    } finally {
        if ($savedDir) {
            Set-Location $savedDir
        }
    }
}

function Verify-KernelBinaryArtifact {
    param(
        [Parameter(Mandatory=$true)]
        [string]$BinaryPath,
        [string]$ExpectedArch = "",
        [Nullable[datetime]]$MinFreshnessUtc = $null
    )

    $resolved = [System.IO.Path]::GetFullPath($BinaryPath)
    if (-not (Test-Path -LiteralPath $resolved)) {
        throw "Verified build artifact path does not exist on disk: $resolved"
    }

    $item = Get-Item -LiteralPath $resolved
    if ($item.Length -eq 0) {
        throw "Verified build artifact is empty (0 bytes): $resolved"
    }
    if ($item.Length -lt 64) {
        throw "Verified build artifact is truncated (smaller than 64-byte ELF header): $resolved ($($item.Length) bytes)"
    }

    if ($MinFreshnessUtc -ne $null -and $item.LastWriteTimeUtc -lt $MinFreshnessUtc) {
        throw "Verified build artifact is stale at $($resolved): last written $($item.LastWriteTimeUtc.ToString('o')), expected at or after $($MinFreshnessUtc.ToString('o'))"
    }

    $fs = [System.IO.File]::OpenRead($resolved)
    try {
        $header = New-Object byte[] 64
        $readBytes = $fs.Read($header, 0, 64)
        if ($readBytes -lt 64) {
            throw "Failed reading complete 64-byte ELF header from $resolved"
        }

        # Check ELF magic: 0x7F, 'E', 'L', 'F'
        if ($header[0] -ne 0x7F -or $header[1] -ne 0x45 -or $header[2] -ne 0x4C -or $header[3] -ne 0x46) {
            throw "Artifact at $resolved is not a valid ELF binary (invalid magic header)"
        }

        # Check 64-bit class (EI_CLASS == 2)
        if ($header[4] -ne 2) {
            throw "Artifact at $resolved is not a 64-bit ELF binary (EI_CLASS=$($header[4]))"
        }

        # Check little-endian data (EI_DATA == 1)
        if ($header[5] -ne 1) {
            throw "Artifact at $resolved is not a little-endian ELF binary (EI_DATA=$($header[5]))"
        }

        # Check architecture if specified (e_machine at offset 18, 16-bit little endian)
        if ($ExpectedArch) {
            $eMachine = [BitConverter]::ToUInt16($header, 18)
            $expectedMachine = switch ($ExpectedArch.ToLower()) {
                "x86_64"  { 0x003E }
                "x86-64"  { 0x003E }
                "amd64"   { 0x003E }
                "aarch64" { 0x00B7 }
                "arm64"   { 0x00B7 }
                "riscv64" { 0x00F3 }
                default   { throw "Unknown ExpectedArch: $ExpectedArch" }
            }
            if ($eMachine -ne $expectedMachine) {
                $actualName = switch ($eMachine) {
                    0x003E { "x86_64" }
                    0x00B7 { "aarch64" }
                    0x00F3 { "riscv64" }
                    0x0003 { "x86_32" }
                    0x0028 { "arm32" }
                    default { "0x$($eMachine.ToString('X4'))" }
                }
                throw "Artifact at $resolved has architecture $actualName (e_machine=0x$($eMachine.ToString('X4'))), expected $ExpectedArch"
            }
        }
    } finally {
        $fs.Close()
    }

    return $resolved
}

function Verify-BiosImageArtifact {
    param(
        [Parameter(Mandatory=$true)]
        [string]$ImagePath,
        [string]$KernelBinaryPath = "",
        [Nullable[datetime]]$MinFreshnessUtc = $null
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

    # Verify kernel binary encapsulation and freshness when KernelBinaryPath is supplied
    if ($KernelBinaryPath) {
        $resolvedKernel = [System.IO.Path]::GetFullPath($KernelBinaryPath)
        if (-not (Test-Path -LiteralPath $resolvedKernel)) {
            throw "Build artifact verification failed: referenced kernel binary not found at $resolvedKernel"
        }
        $kernelItem = Get-Item -LiteralPath $resolvedKernel
        if ($item.Length -le $kernelItem.Length) {
            throw "Build artifact verification failed: BIOS image ($($item.Length) bytes) is smaller than or equal to kernel binary ($($kernelItem.Length) bytes); cannot encapsulate bootloader and kernel"
        }
        if ($item.LastWriteTimeUtc -lt $kernelItem.LastWriteTimeUtc) {
            throw "Build artifact verification failed: BIOS image ($($item.LastWriteTimeUtc.ToString('o'))) is older than kernel binary ($($kernelItem.LastWriteTimeUtc.ToString('o'))); stale image would boot mismatched kernel"
        }
    }

    if ($MinFreshnessUtc -ne $null -and $item.LastWriteTimeUtc -lt $MinFreshnessUtc) {
        throw "Build artifact verification failed: BIOS image at $resolved is stale (written $($item.LastWriteTimeUtc.ToString('o')), required at or after $($MinFreshnessUtc.ToString('o')))"
    }

    Write-Host "[OK] BIOS image verified: $resolved ($($item.Length) bytes)" -ForegroundColor Green
    return $resolved
}
