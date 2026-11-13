#Requires -Version 5.1
<#
.SYNOPSIS
    Download and install the latest lvpm release for Windows.

.DESCRIPTION
    Resolves the latest release of lvpm on GitHub, downloads the Windows x64
    MSI over HTTPS, verifies its SHA-256, and runs it quietly with msiexec.
    The MSI installs per-user (no elevation) to %LOCALAPPDATA%\lvpm\bin,
    adds that folder to the user PATH, and shows up under Settings > Apps,
    where it is uninstalled; a later run upgrades it in place.

    A portable install extracts the release zip instead and copies
    lvpm.exe and relink-package.vi to -InstallDir, with no uninstall
    entry. It is used with -Portable, -InstallDir, -Archive or
    -BuildLocal, and for releases before v0.3.1, which have no MSI.

    The download is verified against every reference available before
    anything is extracted or run:
      - the SHA-256 digest GitHub records for the release asset,
      - a SHA256SUMS, sha256.sum or <asset>.sha256 file from the release,
      - a hash you pass with -Sha256, from a source other than GitHub.
    If none of those is available the script stops. The only things it
    runs are msiexec on the verified MSI and lvpm --version at the end.

    No sign-in is needed.

    Files are unblocked (Windows' mark-of-the-web is removed), so
    SmartScreen and LabVIEW do not treat them as untrusted.

.PARAMETER Version
    Release tag to install, e.g. v0.2.0. Default: the newest release.

.PARAMETER Stable
    Skip pre-releases when picking the newest release. By default
    pre-releases count, because lvpm is still in alpha.

.PARAMETER InstallDir
    Where lvpm.exe goes. Default: %LOCALAPPDATA%\lvpm\bin. Giving it
    makes the install portable; the MSI always uses the default.

.PARAMETER Portable
    Install from the zip instead of the MSI: files only, no uninstall
    entry.

.PARAMETER Sha256
    Expected SHA-256 of the MSI (or of the zip, for a portable install),
    obtained out of band (release notes, the maintainer). When given, the
    download must match it.

.PARAMETER NoPath
    Leave the user PATH as it is. A first MSI install leaves out its PATH
    feature; an upgrade keeps whatever the installed MSI has; an entry
    from an earlier portable install stays.

.PARAMETER Repo
    GitHub repository as owner/name. Default: labview-package-manager/lvpm

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
    .\install.ps1 -Portable

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
    [switch]$Portable,
    [switch]$NoPath,
    [ValidatePattern('^[\w.-]+/[\w.-]+$')]
    [string]$Repo = 'labview-package-manager/lvpm',
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
        # lvpm looks for its relink VI beside itself, as a release ships it.
        Copy-Item -LiteralPath (Join-Path (Split-Path $manifest -Parent) 'src\lv-src\relink-package.vi') -Destination $payload
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

# dist names the zip for the target (lvpm-x86_64-pc-windows-msvc.zip); releases
# before v0.3.1 named it for the version (lvpm-0.2.0-windows-x64.zip).
$assetPatterns = @('lvpm-x86_64-pc-windows-msvc.zip', 'lvpm-*-windows-x64.zip')
# The per-user MSI, from v0.3.1 on (wix/main.wxs).
$msiAsset = 'lvpm-x86_64-pc-windows-msvc.msi'
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

$useMsi = $false
if (-not $Archive) {
    # The MSI unless a portable install was asked for: a custom -InstallDir is
    # one, since the MSI has a single location.
    if (-not $Portable -and -not $PSBoundParameters.ContainsKey('InstallDir')) {
        $asset = $release.assets | Where-Object { $_.name -eq $msiAsset } | Select-Object -First 1
        $useMsi = [bool]$asset
        if (-not $asset) {
            Write-Host "release $($release.tag_name) has no $msiAsset; installing from the zip"
        }
    }
    if (-not $useMsi) {
        $asset = $assetPatterns |
            ForEach-Object { $p = $_; $release.assets | Where-Object { $_.name -like $p } } |
            Select-Object -First 1
    }
    if (-not $asset) {
        throw "release $($release.tag_name) has no asset matching $($assetPatterns -join ' or ')"
    }
}
Write-Host "release: $($release.tag_name)$(if ($release.prerelease) { ' (pre-release)' })"
Write-Host "asset:   $($asset.name) ($([math]::Round($asset.size / 1MB, 1)) MB)"

# ----------------------------------------------------------- user PATH ----

# The user PATH as stored, not as expanded: [Environment]'s getter expands
# %USERPROFILE%-style entries and its setter would write them back expanded,
# as REG_SZ instead of REG_EXPAND_SZ.
function Get-UserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    try {
        if ($key -and ($key.GetValueNames() -contains 'Path')) {
            $raw = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $kind = $key.GetValueKind('Path')
        } else {
            $raw = ''
            $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
        }
    } finally { if ($key) { $key.Close() } }
    [pscustomobject]@{ Parts = @("$raw" -split ';' | Where-Object { $_ }); Kind = $kind }
}

function Set-UserPath([string[]]$Parts, [Microsoft.Win32.RegistryValueKind]$Kind) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try { $key.SetValue('Path', ($Parts -join ';'), $Kind) } finally { $key.Close() }
    # Setting any user variable through [Environment] broadcasts
    # WM_SETTINGCHANGE, so terminals started from Explorer see the new PATH.
    [Environment]::SetEnvironmentVariable('LVPM_INSTALL_REFRESH', '1', 'User')
    [Environment]::SetEnvironmentVariable('LVPM_INSTALL_REFRESH', $null, 'User')
}

# One folder however it is spelled: the MSI writes [Bin] with a trailing
# backslash, this script without.
function Test-SameDir([string]$A, [string]$B) {
    $A.TrimEnd('\') -ieq $B.TrimEnd('\')
}

# ------------------------------------------------------------ download ----

$work = Join-Path ([IO.Path]::GetTempPath()) ("lvpm-install-" + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Path $work | Out-Null

try {
    $download = Join-Path $work $asset.name
    # With Accept: octet-stream the asset API URL redirects to a short-lived
    # signed download URL.
    $dl = $headers.Clone()
    $dl['Accept'] = 'application/octet-stream'
    if ($Archive) {
        Copy-Item -LiteralPath $archiveFile.FullName -Destination $download
    } else {
        Invoke-WebRequest -Uri $asset.url -Headers $dl -OutFile $download -MaximumRedirection 5
    }

    # -------------------------------------------------------- verify ----

    $actual = (Get-FileHash -Algorithm SHA256 -Path $download).Hash.ToLowerInvariant()
    $expected = @()

    if ($Sha256) {
        $expected += [pscustomobject]@{ Source = '-Sha256 parameter'; Hash = $Sha256.ToLowerInvariant() }
    }
    if (($asset.PSObject.Properties.Name -contains 'digest') -and $asset.digest -like 'sha256:*') {
        $expected += [pscustomobject]@{ Source = 'GitHub asset digest'; Hash = $asset.digest.Substring(7).ToLowerInvariant() }
    }
    $sums = $release.assets |
        Where-Object { $_.name -in @('SHA256SUMS', 'SHA256SUMS.txt', 'sha256.sum', "$($asset.name).sha256") } |
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
              "Pass -Sha256 <hash> from a source you trust, or stop here. Downloaded file: $download"
    }
    foreach ($e in $expected) {
        if ($e.Hash -ne $actual) {
            throw "SHA-256 mismatch against $($e.Source): expected $($e.Hash), got $actual. Nothing was installed."
        }
        Write-Host "sha256:  ok ($($e.Source))"
    }

    # ------------------------------------------------------- install ----

    Unblock-File -Path $download

    if ($useMsi) {
        # Already installed by the MSI? It records its files under
        # HKCU\Software\lvpm. If not, a portable install may have left files
        # in the same folder, and a PATH entry the MSI would add a second
        # time: they are taken over, so the MSI owns the folder and its
        # uninstall leaves nothing behind. Both are only moved aside until
        # msiexec has succeeded, and put back if it fails.
        $managed = [bool](Get-ItemProperty -Path 'HKCU:\Software\lvpm' -Name 'lvpm.exe' -ErrorAction SilentlyContinue)
        $before = Get-UserPath
        $aside = Join-Path $work 'previous'
        if (-not $managed -and (Test-Path -LiteralPath $InstallDir)) {
            New-Item -ItemType Directory -Path $aside | Out-Null
            foreach ($name in @('lvpm.exe', 'relink-package.vi', 'README.md', 'LICENSE-MIT', 'LICENSE-APACHE')) {
                $file = Join-Path $InstallDir $name
                if (Test-Path -LiteralPath $file) { Move-Item -LiteralPath $file -Destination $aside }
            }
            # -NoPath leaves PATH as it is, the portable entry included.
            if (-not $NoPath) {
                $kept = @($before.Parts | Where-Object { -not (Test-SameDir $_ $InstallDir) })
                if ($kept.Count -ne $before.Parts.Count) { Set-UserPath $kept $before.Kind }
            }
        }

        # Outside $work, so it survives the cleanup when something fails.
        $log = Join-Path ([IO.Path]::GetTempPath()) 'lvpm-install-msiexec.log'
        $msiArgs = @('/i', "`"$download`"", '/qn', '/norestart', '/l*v', "`"$log`"")
        # Name the features: on an upgrade the MSI otherwise keeps the ones the
        # installed version had, so one -NoPath run would stick. With -NoPath
        # and the MSI already installed, its PATH feature stays as it is.
        if (-not $NoPath) {
            $msiArgs += 'ADDLOCAL=Binaries,Environment'
        } elseif (-not $managed) {
            $msiArgs += 'ADDLOCAL=Binaries'
        }
        $msi = Start-Process -FilePath 'msiexec.exe' -ArgumentList $msiArgs -Wait -PassThru
        # 3010: success, a restart is needed to finish (not expected here).
        if ($msi.ExitCode -notin @(0, 3010)) {
            if (Test-Path -LiteralPath $aside) {
                Get-ChildItem -LiteralPath $aside | Move-Item -Destination $InstallDir -Force
                Set-UserPath $before.Parts $before.Kind
                Write-Host "restored the previous install in $InstallDir"
            }
            throw "msiexec failed with exit code $($msi.ExitCode); its log is $log"
        }
        Write-Host "install: $InstallDir (MSI; uninstall under Settings > Apps)"
        # What PATH now says, not what was asked for.
        if (@((Get-UserPath).Parts | Where-Object { Test-SameDir $_ $InstallDir }).Count -gt 0) {
            if (-not @($env:Path -split ';' | Where-Object { $_ -and (Test-SameDir $_ $InstallDir) })) {
                $env:Path = "$env:Path;$InstallDir"
            }
            Write-Host "path:    $InstallDir is on the user PATH (new terminals will see it)"
        } else {
            Write-Host "path:    $InstallDir is not on the user PATH"
        }
    } else {
        $stage = Join-Path $work 'unpacked'
        Expand-Archive -Path $download -DestinationPath $stage -Force
        $exe = Get-ChildItem -Path $stage -Recurse -File -Filter 'lvpm.exe' | Select-Object -First 1
        if (-not $exe) { throw "$($asset.name) contains no lvpm.exe" }
        if (-not (Test-Path -LiteralPath (Join-Path $exe.DirectoryName 'relink-package.vi'))) {
            Write-Warning "$($asset.name) has no relink-package.vi next to lvpm.exe; releases from v0.3.1 on need it to relink"
        }

        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        Copy-Item -Path (Join-Path $exe.DirectoryName '*') -Destination $InstallDir -Recurse -Force
        Get-ChildItem -Path $InstallDir -Recurse -File | Unblock-File
        Write-Host "install: $InstallDir (portable)"
    }

    if (-not $NoPath -and -not $useMsi) {
        $userPath = Get-UserPath
        if (-not @($userPath.Parts | Where-Object { Test-SameDir $_ $InstallDir })) {
            Set-UserPath (@($userPath.Parts) + $InstallDir) $userPath.Kind
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
