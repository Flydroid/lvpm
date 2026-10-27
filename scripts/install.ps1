#Requires -Version 5.1
<#
.SYNOPSIS
    Download and install the latest lvpm release for Windows.

.DESCRIPTION
    Resolves the latest release of lvpm on GitHub, downloads the Windows x64
    zip over HTTPS, verifies its SHA-256, and installs lvpm.exe.

    The download is verified against every reference available before
    anything is extracted:
      - the SHA-256 digest GitHub records for the release asset,
      - a SHA256SUMS file, when the release ships one,
      - a hash you pass with -Sha256, from a source other than GitHub.
    If none of those is available the script stops. It never runs anything
    it downloaded; lvpm.exe is only invoked with --version at the very end,
    after verification.

    No sign-in is needed.

    Files are unblocked after extraction (Windows' mark-of-the-web is
    removed), so SmartScreen and LabVIEW do not treat them as untrusted.

.PARAMETER Version
    Release tag to install, e.g. v0.2.0. Default: the newest release.

.PARAMETER Stable
    Skip pre-releases when picking the newest release. By default
    pre-releases count, because lvpm is still in alpha.

.PARAMETER InstallDir
    Where lvpm.exe goes. Default: %LOCALAPPDATA%\lvpm\bin

.PARAMETER Sha256
    Expected SHA-256 of the zip, obtained out of band (release notes, the
    maintainer). When given, the download must match it.

.PARAMETER NoPath
    Do not add InstallDir to the user PATH.

.PARAMETER Repo
    GitHub repository as owner/name. Default: Flydroid/lvpm

.PARAMETER BuildLocal
    Build this checkout with cargo build --release --locked, package it,
    and install using the same verification and extraction as a release.
    Requires Rust and the Windows C++ build tools. Does not publish anything.

.PARAMETER Archive
    Install a local release zip instead of downloading from GitHub.
    Requires -Sha256. Cannot be combined with -BuildLocal or release selection.

.EXAMPLE
    .\install.ps1

.EXAMPLE
    .\install.ps1 -BuildLocal

.EXAMPLE
    .\install.ps1 -Version v0.2.0 -InstallDir C:\tools\lvpm -NoPath

.EXAMPLE
    .\install.ps1 -Sha256 1a86ce51553b705566cc1071a8e1c7cd8acc2df664d2e14b6ea46beba3799931
#>
[CmdletBinding()]
param(
    [string]$Version,
    [switch]$Stable,
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'lvpm\bin'),
    [ValidatePattern('^$|^[0-9a-fA-F]{64}$')]
    [string]$Sha256,
    [switch]$NoPath,
    [ValidatePattern('^[\w.-]+/[\w.-]+$')]
    [string]$Repo = 'Flydroid/lvpm',
    [switch]$BuildLocal,
    [string]$Archive
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

if (($BuildLocal -or $Archive) -and ($Version -or $Stable -or $PSBoundParameters.ContainsKey('Repo'))) {
    throw 'Local installation cannot be combined with -Version, -Stable, or -Repo.'
}
if ($BuildLocal -and ($Archive -or $Sha256)) {
    throw '-BuildLocal cannot be combined with -Archive or -Sha256.'
}
if ($Archive -and -not $Sha256) {
    throw '-Archive requires -Sha256.'
}

if ($BuildLocal) {
    $manifest = Join-Path (Split-Path $PSScriptRoot -Parent) 'Cargo.toml'
    & cargo build --release --locked --manifest-path $manifest
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
    $metadataJson = & cargo metadata --no-deps --format-version 1 --locked --manifest-path $manifest
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed with exit code $LASTEXITCODE" }
    $metadata = ($metadataJson -join "`n") | ConvertFrom-Json
    $package = $metadata.packages | Where-Object { $_.name -eq 'lvpm' } | Select-Object -First 1
    if (-not $package) { throw 'cargo metadata contains no lvpm package' }
    $binary = Join-Path $metadata.target_directory 'release\lvpm.exe'
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
        throw "No Windows release executable at $binary. Build with the default Windows host target."
    }
    $bundle = Join-Path ([IO.Path]::GetTempPath()) ("lvpm-local-" + [guid]::NewGuid().ToString('n'))
    New-Item -ItemType Directory -Path $bundle | Out-Null
    try {
        $payload = Join-Path $bundle 'payload'
        New-Item -ItemType Directory -Path $payload | Out-Null
        Copy-Item -LiteralPath $binary -Destination $payload
        foreach ($name in @('README.md', 'LICENSE-MIT', 'LICENSE-APACHE')) {
            Copy-Item -LiteralPath (Join-Path (Split-Path $manifest -Parent) $name) -Destination $payload
        }
        $localZip = Join-Path $bundle "lvpm-$($package.version)-windows-local.zip"
        Compress-Archive -Path (Join-Path $payload '*') -DestinationPath $localZip
        $localHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $localZip).Hash
        & $PSCommandPath -Archive $localZip -Sha256 $localHash -InstallDir $InstallDir -NoPath:$NoPath
    }
    finally {
        Remove-Item -LiteralPath $bundle -Recurse -Force -ErrorAction SilentlyContinue
    }
    return
}

# Windows PowerShell 5.1 negotiates TLS 1.0 by default; insist on 1.2 or better.
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch { }

$assetPattern = 'lvpm-*-windows-x64.zip'
$api = "https://api.github.com/repos/$Repo"

$headers = @{
    'Accept'               = 'application/vnd.github+json'
    'X-GitHub-Api-Version' = '2022-11-28'
    'User-Agent'           = 'lvpm-install.ps1'
}

function Invoke-GitHubApi([string]$Path) {
    Invoke-RestMethod -Uri "$api/$Path" -Headers $headers -MaximumRedirection 5
}

# ------------------------------------------------------ resolve release ----

if ($Archive) {
    $archiveFile = Get-Item -LiteralPath $Archive
    if ($archiveFile.PSIsContainer) { throw '-Archive must name a zip file.' }
    $asset = [pscustomobject]@{ name = $archiveFile.Name; size = $archiveFile.Length }
    $release = [pscustomobject]@{ tag_name = 'local'; prerelease = $false; assets = @() }
} elseif ($Version) {
    $release = Invoke-GitHubApi "releases/tags/$Version"
} else {
    # Invoke-RestMethod hands a JSON array back as one object rather than as a
    # stream of them, so piping the call straight into Where-Object tests the
    # whole array instead of each release and matches nothing. Bind it to a
    # variable first; piping a variable enumerates it.
    $releases = Invoke-GitHubApi 'releases?per_page=30'
    $release = @($releases) |
        Where-Object { -not $_.draft -and (-not $Stable -or -not $_.prerelease) } |
        Select-Object -First 1
    if (-not $release) {
        throw "no release found in $Repo" + $(if ($Stable) { ' (every release is a pre-release; drop -Stable)' })
    }
}

if (-not $Archive) {
    $asset = $release.assets | Where-Object { $_.name -like $assetPattern } | Select-Object -First 1
    if (-not $asset) {
        throw "release $($release.tag_name) has no asset matching $assetPattern"
    }
}
Write-Host "release: $($release.tag_name)$(if ($release.prerelease) { ' (pre-release)' })"
Write-Host "asset:   $($asset.name) ($([math]::Round($asset.size / 1MB, 1)) MB)"

# ------------------------------------------------------------ download ----

$work = Join-Path ([IO.Path]::GetTempPath()) ("lvpm-install-" + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Path $work | Out-Null

try {
    $zip = Join-Path $work $asset.name
    # With Accept: octet-stream the asset API URL redirects to a short-lived
    # signed download URL.
    $dl = $headers.Clone()
    $dl['Accept'] = 'application/octet-stream'
    if ($Archive) {
        Copy-Item -LiteralPath $archiveFile.FullName -Destination $zip
    } else {
        Invoke-WebRequest -Uri $asset.url -Headers $dl -OutFile $zip -MaximumRedirection 5
    }

    # -------------------------------------------------------- verify ----

    $actual = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLowerInvariant()
    $expected = @()

    if ($Sha256) {
        $expected += [pscustomobject]@{ Source = '-Sha256 parameter'; Hash = $Sha256.ToLowerInvariant() }
    }
    if (($asset.PSObject.Properties.Name -contains 'digest') -and $asset.digest -like 'sha256:*') {
        $expected += [pscustomobject]@{ Source = 'GitHub asset digest'; Hash = $asset.digest.Substring(7).ToLowerInvariant() }
    }
    $sums = $release.assets |
        Where-Object { $_.name -in @('SHA256SUMS', 'SHA256SUMS.txt', "$($asset.name).sha256") } |
        Select-Object -First 1
    if ($sums) {
        $sumsFile = Join-Path $work $sums.name
        Invoke-WebRequest -Uri $sums.url -Headers $dl -OutFile $sumsFile -MaximumRedirection 5
        $line = Get-Content $sumsFile | Where-Object { $_ -match '^\s*([0-9a-fA-F]{64})\s+\*?(.+?)\s*$' -and $Matches[2] -eq $asset.name }
        if ($line -and $line -match '^\s*([0-9a-fA-F]{64})') {
            $expected += [pscustomobject]@{ Source = $sums.name; Hash = $Matches[1].ToLowerInvariant() }
        } elseif ($sums.name -like '*.sha256' -and (Get-Content $sumsFile -Raw) -match '([0-9a-fA-F]{64})') {
            $expected += [pscustomobject]@{ Source = $sums.name; Hash = $Matches[1].ToLowerInvariant() }
        }
    }

    if ($expected.Count -eq 0) {
        throw "nothing to verify $($asset.name) against: the release has no digest and no SHA256SUMS. " +
              "Pass -Sha256 <hash> from a source you trust, or stop here. Downloaded file: $zip"
    }
    foreach ($e in $expected) {
        if ($e.Hash -ne $actual) {
            throw "SHA-256 mismatch against $($e.Source): expected $($e.Hash), got $actual. Nothing was installed."
        }
        Write-Host "sha256:  ok ($($e.Source))"
    }

    # ------------------------------------------------------- install ----

    Unblock-File -Path $zip
    $stage = Join-Path $work 'unpacked'
    Expand-Archive -Path $zip -DestinationPath $stage -Force
    $exe = Get-ChildItem -Path $stage -Recurse -File -Filter 'lvpm.exe' | Select-Object -First 1
    if (-not $exe) { throw "$($asset.name) contains no lvpm.exe" }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -Path (Join-Path $exe.DirectoryName '*') -Destination $InstallDir -Recurse -Force
    Get-ChildItem -Path $InstallDir -Recurse -File | Unblock-File
    Write-Host "install: $InstallDir"

    if (-not $NoPath) {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $parts = @($userPath -split ';' | Where-Object { $_ })
        if ($parts -notcontains $InstallDir) {
            [Environment]::SetEnvironmentVariable('Path', (($parts + $InstallDir) -join ';'), 'User')
            $env:Path = "$env:Path;$InstallDir"
            Write-Host "path:    added to the user PATH (new terminals will see it)"
        } else {
            Write-Host "path:    already on the user PATH"
        }
    }

    Write-Host ''
    $installed = (& (Join-Path $InstallDir 'lvpm.exe') --version) -join ' '
    if ($LASTEXITCODE -ne 0) { throw "Installed lvpm --version failed with exit code $LASTEXITCODE" }
    Write-Host "$installed installed"
}
finally {
    Remove-Item -Path $work -Recurse -Force -ErrorAction SilentlyContinue
}
