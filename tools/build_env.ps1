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

function Get-ElfLoadSegmentGeometry {
    <#
    .SYNOPSIS
    Reads the PT_LOAD file extents an ELF declares for itself.

    .DESCRIPTION
    This mirrors the geometry walk in kernel/src/storage/boot_guard.rs: bounds
    e_phnum, e_phentsize and the program-header table span so corrupt values
    cannot turn the parse into a wild read. Returning the segments is what lets
    the caller compare the bytes a bootloader actually embedded rather than
    trusting a file size.
    #>
    param(
        [Parameter(Mandatory=$true)][System.IO.FileStream]$Stream,
        [Parameter(Mandatory=$true)][string]$Label
    )

    $header = New-Object byte[] 64
    if ($Stream.Read($header, 0, 64) -lt 64) { throw "$Label is too short to contain a 64-byte ELF header" }
    if ($header[0] -ne 0x7F -or $header[1] -ne 0x45 -or $header[2] -ne 0x4C -or $header[3] -ne 0x46) {
        throw "$Label does not begin with the ELF magic"
    }
    if ($header[4] -ne 2) { throw "$Label is not a 64-bit ELF (EI_CLASS=$($header[4]))" }
    if ($header[5] -ne 1) { throw "$Label is not a little-endian ELF (EI_DATA=$($header[5]))" }

    $eMachine = [BitConverter]::ToUInt16($header, 18)
    $phOff = [BitConverter]::ToUInt64($header, 32)
    $phEntSize = [BitConverter]::ToUInt16($header, 54)
    $phNum = [BitConverter]::ToUInt16($header, 56)

    # Bounds matching kernel/src/storage/boot_guard.rs.
    if ($phNum -lt 1 -or $phNum -gt 64) { throw "$Label declares $phNum program headers, outside the accepted range 1..64" }
    if ($phEntSize -lt 56 -or $phEntSize -gt 512) { throw "$Label declares a $phEntSize-byte program header entry, outside the accepted range 56..512" }
    if ($phOff -gt 0x10000) { throw "$Label places its program header table at $phOff, beyond the accepted bound" }

    $entry = New-Object byte[] $phEntSize
    $segments = @()
    for ($i = 0; $i -lt $phNum; $i++) {
        $offset = [int64]$phOff + [int64]$i * [int64]$phEntSize
        if ($offset + $phEntSize -gt $Stream.Length) { throw "$Label has a program header table extending past its own end" }
        $null = $Stream.Seek($offset, [System.IO.SeekOrigin]::Begin)
        if ($Stream.Read($entry, 0, $phEntSize) -lt $phEntSize) { throw "$Label truncated while reading program header $i" }
        $pType = [BitConverter]::ToUInt32($entry, 0)
        if ($pType -ne 1) { continue }   # PT_LOAD
        $pOffset = [BitConverter]::ToUInt64($entry, 8)
        $pFileSize = [BitConverter]::ToUInt64($entry, 32)
        $pMemSize = [BitConverter]::ToUInt64($entry, 40)
        if ($pFileSize -eq 0) { continue }
        if (($pOffset + $pFileSize) -gt [uint64]$Stream.Length) { throw "$Label declares a PT_LOAD segment that extends past its own end" }
        $segments += [pscustomobject]@{ Offset = $pOffset; FileSize = $pFileSize; MemSize = $pMemSize }
    }
    if ($segments.Count -eq 0) { throw "$Label declares no PT_LOAD segment, so it has no loadable contents to embed" }

    return [pscustomobject]@{ Machine = $eMachine; Segments = $segments }
}

function Assert-NoSymbolTable {
    <#
    .SYNOPSIS
    Fails when an ELF still carries .symtab or .strtab.

    .DESCRIPTION
    The symbol table is the one part of a userspace image that is not a function of
    the source. LLVM uniquifies internal-linkage symbols as "<name>.llvm.<hash>",
    where the hash is derived from the module identifier -- the absolute source path
    -- so the same revision built from two different directories produces two
    different byte images even though every loadable section matches. Stripping the
    table is therefore what makes the artifact reproducible, and this asserts the
    strip actually happened instead of trusting the exit code.

    Walks the section header table the same defensive way as
    Get-ElfLoadSegmentGeometry: bounds e_shentsize, e_shnum and the name lookup so
    corrupt values cannot turn the parse into a wild read.
    #>
    param(
        [Parameter(Mandatory=$true)][string]$Path,
        [string]$Label
    )
    if (-not $Label) { $Label = $Path }

    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 64) { throw "[FAILED] $Label is too short to hold an ELF header" }
    if ($bytes[0] -ne 0x7F -or $bytes[1] -ne 0x45 -or $bytes[2] -ne 0x4C -or $bytes[3] -ne 0x46) {
        throw "[FAILED] $Label does not begin with the ELF magic"
    }
    if ($bytes[4] -ne 2) { throw "[FAILED] $Label is not a 64-bit ELF, so the section table layout below does not apply" }

    $eShoff = [BitConverter]::ToUInt64($bytes, 40)
    $eShEntSize = [int][BitConverter]::ToUInt16($bytes, 58)
    $eShNum = [int][BitConverter]::ToUInt16($bytes, 60)
    $eShStrNdx = [int][BitConverter]::ToUInt16($bytes, 62)
    if ($eShoff -eq 0 -or $eShNum -eq 0) { throw "[FAILED] $Label declares no section header table" }
    if ($eShEntSize -lt 64) { throw "[FAILED] $Label declares an implausible e_shentsize ($eShEntSize)" }
    if (($eShoff + ([uint64]$eShNum * $eShEntSize)) -gt [uint64]$bytes.Length) {
        throw "[FAILED] $Label declares a section header table that extends past its own end"
    }
    if ($eShStrNdx -ge $eShNum) { throw "[FAILED] $Label declares e_shstrndx $eShStrNdx outside its $eShNum sections" }

    $strHdr = [int]$eShoff + ($eShStrNdx * $eShEntSize)
    $strOffset = [int][BitConverter]::ToUInt64($bytes, $strHdr + 24)
    $strSize = [int][BitConverter]::ToUInt64($bytes, $strHdr + 32)
    if ($strSize -lt 0 -or ($strOffset + $strSize) -gt $bytes.Length) {
        throw "[FAILED] $Label declares a section name table that extends past its own end"
    }

    $found = @()
    for ($i = 0; $i -lt $eShNum; $i++) {
        $hdr = [int]$eShoff + ($i * $eShEntSize)
        $nameOffset = [int][BitConverter]::ToUInt32($bytes, $hdr)
        if ($nameOffset -lt 0 -or $nameOffset -ge $strSize) { continue }
        $end = $nameOffset
        while ($end -lt $strSize -and $bytes[$strOffset + $end] -ne 0) { $end++ }
        $name = [System.Text.Encoding]::ASCII.GetString($bytes, $strOffset + $nameOffset, $end - $nameOffset)
        if ($name -eq ".symtab" -or $name -eq ".strtab") { $found += $name }
    }

    if ($found.Count -gt 0) {
        throw ("[FAILED] $Label still carries $($found -join ' and '); rebuilding elsewhere would produce a different image")
    }
}

function Get-NewestUserspaceSourceUtc {
    <#
    .SYNOPSIS
    Newest modification time among the sources that produce the Ring-3 images.

    .DESCRIPTION
    Returns a UTC DateTime, or $null when no source could be found.

    Scopes are exactly the trees that build the images the kernel embeds with
    include_bytes!: `userspace` (init and the system services), `apps`, and the
    `sdk` crate they all link against. The kernel tree is deliberately excluded --
    kernel source does not change what a Ring-3 image contains, and including it
    would force all 26 binaries to be rebuilt and the kernel recompiled every
    time kernel code was touched.

    `tools` and the build scripts are excluded for the same reason, plus one
    more: they are not embedded, so editing this file must not invalidate an
    image that does not contain it.
    #>
    param([string]$Root)

    if (-not $Root) { $Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path) }

    $newest = $null
    foreach ($scope in @("userspace", "apps", "sdk")) {
        $base = Join-Path $Root $scope
        if (-not (Test-Path -LiteralPath $base)) { continue }
        $files = Get-ChildItem -LiteralPath $base -Recurse -File -ErrorAction SilentlyContinue |
            Where-Object {
                $_.Extension -in @(".rs", ".toml", ".ld") -and
                $_.FullName -notmatch '\\target\\' -and
                $_.FullName -notmatch '\\node_modules\\'
            }
        foreach ($file in $files) {
            $stamp = $file.LastWriteTimeUtc
            if ($newest -eq $null -or $stamp -gt $newest) { $newest = $stamp }
        }
    }
    return $newest
}

function Get-StreamRegionSha256 {
    <#
    .SYNOPSIS
    SHA-256 of a byte range of an open stream, read in bounded chunks.

    .DESCRIPTION
    Comparing digests rather than bytes keeps a multi-megabyte kernel cheap to
    verify in PowerShell, and reading in 1 MiB chunks avoids materialising a
    second copy of the payload. A short read is an error rather than a partial
    hash, so a truncated region can never compare equal to anything.
    #>
    param(
        [Parameter(Mandatory=$true)][System.IO.FileStream]$Stream,
        [Parameter(Mandatory=$true)][int64]$Offset,
        [Parameter(Mandatory=$true)][int64]$Length
    )

    if ($Length -le 0) { return "empty" }
    $null = $Stream.Seek($Offset, [System.IO.SeekOrigin]::Begin)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $buffer = New-Object byte[] 1048576
    $remaining = $Length
    while ($remaining -gt 0) {
        $want = [int][Math]::Min([int64]$buffer.Length, $remaining)
        $read = $Stream.Read($buffer, 0, $want)
        if ($read -le 0) { $sha.Dispose(); throw "unexpected end of stream while hashing $Length bytes at offset $Offset" }
        $null = $sha.TransformBlock($buffer, 0, $read, $null, 0)
        $remaining -= $read
    }
    $null = $sha.TransformFinalBlock((New-Object byte[] 0), 0, 0)
    $digest = $sha.Hash
    $sha.Dispose()
    return [System.BitConverter]::ToString($digest)
}

function Test-BiosImageEmbedsKernel {
    <#
    .SYNOPSIS
    Proves the bootable image contains the exact loadable bytes of a given kernel.

    .DESCRIPTION
    The previous check compared the image's size against the kernel's size and
    compared modification times. Neither says which kernel is inside: an image
    holding a different kernel that is larger and newer passes both.

    The bootloader crate writes the payload on sector boundaries after its boot
    code, so this scans 512-byte-aligned offsets for a 64-bit little-endian ELF
    header, checks the architecture matches the kernel it was given, and then
    compares every PT_LOAD file extent byte-for-byte by digest.

    The scan is deliberately restricted to sector-aligned offsets, which is the
    layout the bootloader actually produces. If a future bootloader changes
    that, this fails loudly with the offsets it examined rather than reporting a
    pass it cannot justify.
    #>
    param(
        [Parameter(Mandatory=$true)][string]$ImagePath,
        [Parameter(Mandatory=$true)][string]$KernelBinaryPath
    )

    $imageStream = [System.IO.File]::Open($ImagePath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    $kernelStream = [System.IO.File]::Open($KernelBinaryPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try {
        $kernel = Get-ElfLoadSegmentGeometry -Stream $kernelStream -Label "kernel binary $KernelBinaryPath"
        $imageLength = $imageStream.Length

        # Digest the kernel's own segments once.
        $expected = @()
        foreach ($seg in $kernel.Segments) {
            $expected += (Get-StreamRegionSha256 -Stream $kernelStream -Offset ([int64]$seg.Offset) -Length ([int64]$seg.FileSize))
        }
        # A cheap discriminator so a wrong candidate is rejected without
        # hashing its whole payload. This never decides a match on its own.
        $probeLength = [int64][Math]::Min(4096, [int64]$kernel.Segments[0].FileSize)
        $kernelProbe = Get-StreamRegionSha256 -Stream $kernelStream -Offset 0 -Length $probeLength

        $imageHeader = New-Object byte[] 64
        $magic = [byte[]]@(0x7F, 0x45, 0x4C, 0x46)
        $examined = 0
        for ($base = 0L; $base -lt $imageLength; $base += 512) {
            $examined++
            $null = $imageStream.Seek($base, [System.IO.SeekOrigin]::Begin)
            if ($imageStream.Read($imageHeader, 0, 64) -lt 64) { continue }
            if ($imageHeader[0] -ne $magic[0] -or $imageHeader[1] -ne $magic[1] -or
                $imageHeader[2] -ne $magic[2] -or $imageHeader[3] -ne $magic[3]) { continue }
            if ($imageHeader[4] -ne 2 -or $imageHeader[5] -ne 1) { continue }
            if ([BitConverter]::ToUInt16($imageHeader, 18) -ne $kernel.Machine) { continue }

            $imageProbe = Get-StreamRegionSha256 -Stream $imageStream -Offset $base -Length $probeLength
            if ($imageProbe -ne $kernelProbe) { continue }

            # Full comparison of every PT_LOAD extent.
            $allMatch = $true
            for ($i = 0; $i -lt $kernel.Segments.Count; $i++) {
                $seg = $kernel.Segments[$i]
                $end = $base + [int64]$seg.Offset + [int64]$seg.FileSize
                if ($end -gt $imageLength) { $allMatch = $false; break }
                $actual = Get-StreamRegionSha256 -Stream $imageStream -Offset ($base + [int64]$seg.Offset) -Length ([int64]$seg.FileSize)
                if ($actual -ne $expected[$i]) { $allMatch = $false; break }
            }
            if ($allMatch) {
                Write-Host "[OK] BIOS image embeds the intended kernel at sector offset $base ($($kernel.Segments.Count) PT_LOAD segments verified)." -ForegroundColor Green
                return $base
            }
        }
        throw ("Build artifact verification failed: no sector-aligned ELF in $ImagePath embeds the loadable bytes of $KernelBinaryPath " +
               "(examined $examined sector-aligned offsets). The image holds a different kernel, or its payload is not sector aligned.")
    } finally {
        $imageStream.Close()
        $kernelStream.Close()
    }
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

    $fs = $null
    for ($attempt = 0; $attempt -lt 10; $attempt++) {
        try {
            $fs = [System.IO.File]::Open($resolved, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
            break
        } catch [System.IO.IOException] {
            Start-Sleep -Milliseconds 50
        }
    }
    if ($fs -eq $null) {
        $fs = [System.IO.File]::Open($resolved, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    }
    try {
        $sector = New-Object byte[] 512
        $readBytes = $fs.Read($sector, 0, 512)
        if ($readBytes -lt 512 -or $sector[510] -ne 0x55 -or $sector[511] -ne 0xAA) {
            throw "Build artifact verification failed: BIOS image at $resolved lacks valid MBR boot signature (0x55, 0xAA)"
        }
    } finally {
        $fs.Close()
    }

    # Verify kernel binary encapsulation when KernelBinaryPath is supplied.
    if ($KernelBinaryPath) {
        $resolvedKernel = [System.IO.Path]::GetFullPath($KernelBinaryPath)
        if (-not (Test-Path -LiteralPath $resolvedKernel)) {
            throw "Build artifact verification failed: referenced kernel binary not found at $resolvedKernel"
        }
        $kernelItem = Get-Item -LiteralPath $resolvedKernel
        if ($item.Length -le $kernelItem.Length) {
            throw "Build artifact verification failed: BIOS image ($($item.Length) bytes) is smaller than or equal to kernel binary ($($kernelItem.Length) bytes); cannot encapsulate bootloader and kernel"
        }
        # Cheap staleness guard first, so the common "forgot to repackage" case
        # gets a precise diagnostic instead of a content mismatch. It cannot
        # establish correctness on its own; the content check below does that.
        if ($item.LastWriteTimeUtc -lt $kernelItem.LastWriteTimeUtc) {
            throw "Build artifact verification failed: BIOS image ($($item.LastWriteTimeUtc.ToString('o'))) is older than kernel binary ($($kernelItem.LastWriteTimeUtc.ToString('o'))); stale image would boot a mismatched kernel"
        }
        # The real relationship: the image must actually contain this kernel.
        $null = Test-BiosImageEmbedsKernel -ImagePath $resolved -KernelBinaryPath $resolvedKernel
    }

    if ($MinFreshnessUtc -ne $null -and $item.LastWriteTimeUtc -lt $MinFreshnessUtc) {
        throw "Build artifact verification failed: BIOS image at $resolved is stale (written $($item.LastWriteTimeUtc.ToString('o')), required at or after $($MinFreshnessUtc.ToString('o')))"
    }

    Write-Host "[OK] BIOS image verified: $resolved ($($item.Length) bytes)" -ForegroundColor Green
    return $resolved
}
