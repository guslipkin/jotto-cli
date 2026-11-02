# PowerShell installer script for jotto, wordle, contexto, flights, and arcade (https://github.com/guslipkin/jotto-cli)
#
# Usage:
#   irm https://raw.githubusercontent.com/guslipkin/jotto-cli/main/install.ps1 | iex

$ErrorActionPreference = 'Stop'

$Repo = "guslipkin/jotto-cli"
# The games' commands: in a release, hard links to arcade.exe, which has all four
# in it and runs as whichever game it's named after. (Hard links, not symbolic
# ones, since those need administrator rights or Developer Mode on Windows.)
$Games = @("jotto.exe", "wordle.exe", "contexto.exe", "flights.exe")
$InstallDir = if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { Join-Path $HOME ".local\bin" }
$Version = if ($env:VERSION) { $env:VERSION } else { "latest" }
$AssetName = "windows-x86_64"
$ArchiveFile = "jotto-$AssetName.zip"

if ($Version -eq "latest") {
    $BaseUrl = "https://github.com/$Repo/releases/latest/download"
} else {
    $BaseUrl = "https://github.com/$Repo/releases/download/$Version"
}

$ArchiveUrl = "$BaseUrl/$ArchiveFile"
$ChecksumsUrl = "$BaseUrl/SHA256SUMS.txt"

$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("jotto-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TmpDir | Out-Null

try {
    $ZipPath = Join-Path $TmpDir $ArchiveFile
    Write-Host "Downloading $ArchiveFile ($Version)..."
    Invoke-WebRequest -Uri $ArchiveUrl -OutFile $ZipPath -UseBasicParsing

    try {
        $SumsPath = Join-Path $TmpDir "SHA256SUMS.txt"
        Invoke-WebRequest -Uri $ChecksumsUrl -OutFile $SumsPath -UseBasicParsing
        $ExpectedLine = Get-Content $SumsPath | Where-Object { $_ -match "\s$([regex]::Escape($ArchiveFile))$" } | Select-Object -First 1
        if ($ExpectedLine) {
            $ExpectedHash = ($ExpectedLine -split '\s+')[0].ToLowerInvariant()
            $ActualHash = (Get-FileHash -Path $ZipPath -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($ActualHash -ne $ExpectedHash) {
                throw "SHA-256 checksum mismatch for $ArchiveFile (expected $ExpectedHash, got $ActualHash)."
            }
            Write-Host "Verified SHA-256 checksum."
        }
    } catch {
        if ($_.Exception.Message -match "checksum mismatch") {
            throw
        }
    }

    $ExtractDir = Join-Path $TmpDir "extracted"
    Expand-Archive -Path $ZipPath -DestinationPath $ExtractDir -Force

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

    $Installed = @()
    $Find = { param($Name) Get-ChildItem -Path $ExtractDir -Filter $Name -Recurse -File | Select-Object -First 1 }
    $Arcade = & $Find "arcade.exe"
    $ArcadePath = Join-Path $InstallDir "arcade.exe"
    if ($Arcade) {
        # The old games' links first, so replacing arcade.exe doesn't write
        # through them.
        foreach ($Bin in $Games) {
            Remove-Item -Path (Join-Path $InstallDir $Bin) -Force -ErrorAction SilentlyContinue
        }
        Remove-Item -Path $ArcadePath -Force -ErrorAction SilentlyContinue
        Copy-Item -Path $Arcade.FullName -Destination $ArcadePath -Force
        $Installed += "arcade.exe"
    }

    # Older releases shipped a binary per game (and before v0.0.5, no arcade);
    # those are copied as they are.
    foreach ($Bin in $Games) {
        $Dest = Join-Path $InstallDir $Bin
        $Found = & $Find $Bin
        if ($Found) {
            # Removed first, as it may be a link to an installed arcade.exe.
            Remove-Item -Path $Dest -Force -ErrorAction SilentlyContinue
            Copy-Item -Path $Found.FullName -Destination $Dest -Force
            $Installed += $Bin
        } elseif ($Arcade) {
            try {
                New-Item -ItemType HardLink -Path $Dest -Target $ArcadePath -Force | Out-Null
            } catch {
                # A drive without hard links (FAT32, say) gets a copy.
                Copy-Item -Path $ArcadePath -Destination $Dest -Force
            }
            $Installed += "$Bin (-> arcade.exe)"
        }
    }

    if ($Installed.Count -eq 0) {
        throw "No binaries found in $ArchiveFile."
    }

    Write-Host "Installed: $($Installed -join ', ') -> $InstallDir"

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathParts = ($UserPath -split ';') | Where-Object { $_ -ne '' }
    if ($PathParts -notcontains $InstallDir) {
        [Environment]::SetEnvironmentVariable("Path", (($PathParts + $InstallDir) -join ';'), "User")
        $env:Path = "$InstallDir;$env:Path"
        Write-Host "Added $InstallDir to your user PATH."
    }
} finally {
    if (Test-Path $TmpDir) {
        Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
