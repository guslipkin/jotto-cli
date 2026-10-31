# PowerShell installer script for jotto, wordle, contexto, and flights (https://github.com/guslipkin/jotto-cli)
#
# Usage:
#   irm https://raw.githubusercontent.com/guslipkin/jotto-cli/main/install.ps1 | iex

$ErrorActionPreference = 'Stop'

$Repo = "guslipkin/jotto-cli"
$Binaries = @("jotto.exe", "wordle.exe", "contexto.exe", "flights.exe")
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
    foreach ($Bin in $Binaries) {
        $Found = Get-ChildItem -Path $ExtractDir -Filter $Bin -Recurse -File | Select-Object -First 1
        if ($Found) {
            Copy-Item -Path $Found.FullName -Destination (Join-Path $InstallDir $Bin) -Force
            $Installed += $Bin
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
